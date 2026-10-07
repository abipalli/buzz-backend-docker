//! `buzz-backend-docker`: a Buzz remote-agent backend provider that runs each
//! agent as a container on a Docker host (spec: block/buzz
//! `docs/remote-agents.md`, protocol version 1).

mod config;
mod docker;
mod env;
mod naming;
mod reconcile;
mod wire;

use docker::{DockerCli, RunSpec};
use naming::{AgentIdentity, LABEL_CREATE_INTENT, LABEL_IMAGE};
use sha2::{Digest, Sha256};
use std::io::Read;
use wire::{Request, Response};

const RELAY_MESH_PROVIDER: &str = "relay-mesh";
const STOP_TIMEOUT_SECS: u32 = 60;
const CA_FILE: &str = "/etc/buzz-ca/ca.crt";
/// Adds the private CA alongside the system store; `SSL_CERT_FILE` would
/// replace the store and break public TLS (model APIs, git hosts).
const CA_ENV: &str = "SSL_CERT_DIR";
const CA_ENV_VALUE: &str = "/etc/ssl/certs:/etc/buzz-ca";
/// Host-resolved values that must be re-derived in the image, never forwarded.
const DROPPED_ENV: [&str; 2] = ["PATH", "HOME"];

fn main() {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("could not read the request from stdin: {e}");
        std::process::exit(1);
    }
    let response = respond(&input);
    println!(
        "{}",
        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!(r#"{{"ok":false,"error":"could not serialize a response: {e}"}}"#))
    );
}

fn respond(input: &str) -> Response {
    let raw: serde_json::Value = match serde_json::from_str(input) {
        Ok(value) => value,
        Err(e) => return Response::error(format!("request is not valid JSON: {e}")),
    };
    if let Some(refusal) = refuse_relay_mesh(&raw) {
        return Response::error(refusal);
    }
    let request: Request = match serde_json::from_value(raw) {
        Ok(request) => request,
        Err(e) => return Response::error(format!("could not understand the request: {e}")),
    };
    match request {
        Request::Info => Response::info(),
        Request::Deploy(deploy) => match prepare(&deploy).and_then(|(cfg, identity, desired)| {
            let substrate = DockerCli { context: cfg.context.clone() };
            reconcile::deploy(&substrate, &identity, &desired, &reconcile::Timing::production())
        }) {
            Ok(agent_id) => Response::deployed(agent_id),
            Err(e) => Response::error(e),
        },
    }
}

fn refuse_relay_mesh(raw: &serde_json::Value) -> Option<String> {
    let provider = raw.get("agent")?.get("provider")?.as_str()?;
    (provider.trim() == RELAY_MESH_PROVIDER).then(|| {
        "deploy refused: this agent is configured for shared compute (relay-mesh), which runs \
         on the relay rather than in a container. Switch the agent to a local runtime before \
         deploying it to Docker."
            .to_string()
    })
}

/// Everything decided before the first Docker call: config, identity, the
/// container's environment and shape. Pure, so it is tested without Docker.
fn prepare(request: &wire::DeployRequest) -> Result<(config::ProviderConfig, AgentIdentity, reconcile::Desired), String> {
    let cfg = config::parse(&request.provider_config)?;
    let identity = AgentIdentity::from_nsec(&request.agent.private_key_nsec)?;

    let mut env = env::build_env(
        &request.agent,
        env::AuthoritativeInputs {
            generation: "pending",
            inactivity_seconds: (cfg.inactivity_seconds > 0).then_some(cfg.inactivity_seconds),
        },
    )?;
    for key in DROPPED_ENV {
        env.remove(key);
    }
    if let Some(key) = env.keys().find(|k| docker::cli_reserved(k)) {
        return Err(format!(
            "deploy refused: agent env var {key} would reconfigure the docker CLI on this machine \
             instead of reaching the agent; rename it"
        ));
    }

    let mut mounts = Vec::new();
    if let (Some(volume), Some(subpath)) = (&cfg.ca_volume, &cfg.ca_volume_subpath) {
        mounts.push(format!("type=volume,src={volume},dst={CA_FILE},readonly,volume-subpath={subpath}"));
        env.insert(CA_ENV.to_string(), CA_ENV_VALUE.to_string());
    }

    let intent = serde_json::json!({
        "binding_version": naming::BINDING_VERSION,
        "image": cfg.image,
        "network": cfg.network,
        "add_hosts": cfg.add_hosts,
        "mounts": mounts,
        "cpus": cfg.cpus,
        "memory": cfg.memory,
        "stop_timeout_secs": STOP_TIMEOUT_SECS,
    });
    let mut labels = identity.labels();
    labels.insert(LABEL_CREATE_INTENT.into(), hex::encode(Sha256::digest(intent.to_string())));
    labels.insert(LABEL_IMAGE.into(), cfg.image.clone());

    let spec = RunSpec {
        name: identity.container_name(),
        image: cfg.image.clone(),
        labels,
        network: cfg.network.clone(),
        add_hosts: cfg.add_hosts.clone(),
        mounts,
        cpus: cfg.cpus.clone(),
        memory: cfg.memory.clone(),
        stop_timeout_secs: STOP_TIMEOUT_SECS,
    };
    Ok((cfg, identity, reconcile::Desired { spec, env }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NSEC: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";

    fn deploy_request(agent_extra: serde_json::Value, provider_config: serde_json::Value) -> wire::DeployRequest {
        let mut agent = json!({"relay_url": "wss://relay.example", "private_key_nsec": NSEC, "auth_tag": "tag"});
        agent.as_object_mut().unwrap().extend(agent_extra.as_object().unwrap().clone());
        let Request::Deploy(d) =
            serde_json::from_value(json!({"op": "deploy", "agent": agent, "provider_config": provider_config})).unwrap()
        else {
            panic!("not a deploy")
        };
        *d
    }

    fn prepare_err(req: &wire::DeployRequest) -> String {
        match prepare(req) {
            Err(e) => e,
            Ok(_) => panic!("expected a refusal"),
        }
    }

    #[test]
    fn info_declares_protocol_version_one() {
        let v = serde_json::to_value(respond(r#"{"op":"info","request_id":"r"}"#)).unwrap();
        assert_eq!((v["ok"].clone(), v["protocol_version"].clone(), v["name"].clone()), (json!(true), json!(1), json!("docker")));
        assert!(v["config_schema"]["properties"]["image"].is_object());
    }

    #[test]
    fn bad_input_is_an_in_band_error() {
        for input in ["not json", r#"{"op":"undeploy"}"#] {
            assert_eq!(serde_json::to_value(respond(input)).unwrap()["ok"], json!(false));
        }
    }

    #[test]
    fn relay_mesh_agents_are_refused() {
        let v = serde_json::to_value(respond(
            r#"{"op":"deploy","agent":{"provider":" relay-mesh ","relay_url":"wss://r","private_key_nsec":"x"}}"#,
        ))
        .unwrap();
        assert!(v["error"].as_str().unwrap().contains("relay-mesh"));
    }

    #[test]
    fn malformed_key_is_refused_before_any_docker_call() {
        let req = deploy_request(json!({"private_key_nsec": "nsec1nope"}), json!({}));
        assert!(prepare_err(&req).contains("nsec"));
    }

    #[test]
    fn prepared_container_carries_identity_labels_and_intent() {
        let (_, identity, desired) = prepare(&deploy_request(json!({}), json!({}))).unwrap();
        assert!(identity.owns(&desired.spec.labels));
        assert_eq!(desired.spec.labels[LABEL_CREATE_INTENT].len(), 64);
        assert_eq!(desired.env["BUZZ_ACP_EXIT_AFTER_INACTIVITY"], "7200");
        assert!(!desired.spec.mounts.iter().any(|m| m.contains("volume-subpath")));
    }

    #[test]
    fn zero_inactivity_means_no_reaper_env() {
        let (_, _, desired) = prepare(&deploy_request(json!({}), json!({"inactivity_seconds": 0}))).unwrap();
        assert!(!desired.env.contains_key("BUZZ_ACP_EXIT_AFTER_INACTIVITY"));
    }

    #[test]
    fn host_paths_are_dropped_and_cli_hijacks_refused() {
        let (_, _, desired) =
            prepare(&deploy_request(json!({"env_vars": {"PATH": "/Users/me/bin", "HOME": "/Users/me"}}), json!({}))).unwrap();
        assert!(!desired.env.contains_key("PATH") && !desired.env.contains_key("HOME"));
        for key in ["DOCKER_HOST", "SSH_AUTH_SOCK"] {
            let err = prepare_err(&deploy_request(json!({"env_vars": {key: "x"}}), json!({})));
            assert!(err.contains(key), "{err}");
        }
    }

    #[test]
    fn ca_volume_mounts_only_the_certificate() {
        let (_, _, desired) = prepare(&deploy_request(
            json!({}),
            json!({"ca_volume": "docker_stepca_data", "ca_volume_subpath": "certs/root_ca.crt"}),
        ))
        .unwrap();
        assert_eq!(
            desired.spec.mounts,
            [format!("type=volume,src=docker_stepca_data,dst={CA_FILE},readonly,volume-subpath=certs/root_ca.crt")]
        );
        assert_eq!(desired.env[CA_ENV], CA_ENV_VALUE);
    }

    #[test]
    fn intent_changes_with_shape_not_with_secrets() {
        let a = prepare(&deploy_request(json!({}), json!({}))).unwrap().2.spec.labels[LABEL_CREATE_INTENT].clone();
        let b = prepare(&deploy_request(json!({"env_vars": {"OPENAI_API_KEY": "sk-other"}}), json!({})))
            .unwrap()
            .2
            .spec
            .labels[LABEL_CREATE_INTENT]
            .clone();
        let c = prepare(&deploy_request(json!({}), json!({"memory": "8g"}))).unwrap().2.spec.labels[LABEL_CREATE_INTENT].clone();
        assert_eq!(a, b, "secret values leaked into the fingerprint");
        assert_ne!(a, c, "shape change not reflected");
    }
}
