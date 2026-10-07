//! Deploy = converge to at most one live instance (spec §Deploy State
//! Machine), realized for Docker containers.

use crate::docker::{Observed, RemoveOutcome, RunError, RunSpec, Substrate};
use crate::env::START_NONCE_KEY;
use crate::naming::{new_generation, AgentIdentity};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

pub struct Desired {
    pub spec: RunSpec,
    pub env: BTreeMap<String, String>,
}

pub struct Timing {
    pub deadline: Duration,
    pub poll: Duration,
    /// How long a freshly created container must stay up before the deploy
    /// reports success — the "harness actually started" criterion.
    pub settle: Duration,
}

impl Timing {
    pub fn production() -> Self {
        Self {
            deadline: Duration::from_secs(600),
            poll: Duration::from_secs(1),
            settle: Duration::from_secs(5),
        }
    }
}

const LIVE: [&str; 3] = ["running", "paused", "restarting"];
const TERMINATED: [&str; 2] = ["exited", "dead"];

pub fn deploy(
    sub: &dyn Substrate,
    identity: &AgentIdentity,
    desired: &Desired,
    timing: &Timing,
) -> Result<String, String> {
    let name = desired.spec.name.clone();
    let started = Instant::now();
    let mut attempted = false;
    let mut attempt_error: Option<String> = None;
    let mut settled_since: Option<Instant> = None;

    loop {
        if started.elapsed() > timing.deadline {
            return Err(format!("startup of {name} not confirmed within {}s; the next Start will pick up where it is", timing.deadline.as_secs()));
        }
        let observed = sub.inspect(&name)?;
        let Some(obs) = observed else {
            if attempted {
                return Err(attempt_error
                    .unwrap_or_else(|| format!("{name} disappeared right after it was created")));
            }
            attempted = true;
            match create(sub, desired) {
                Ok(()) => {}
                Err(RunError::Conflict) => {}
                Err(RunError::Failed(msg)) => attempt_error = Some(msg),
            }
            continue;
        };

        if !identity.owns(&obs.labels) {
            return Err(format!(
                "a container named {name} exists but was not created by this provider for this agent; \
                 remove it yourself ({} rm {name}) if it is safe to do so",
                sub.describe()
            ));
        }

        let status = obs.status.as_str();
        if status == "removing" {
            std::thread::sleep(timing.poll);
            continue;
        }
        if LIVE.contains(&status) {
            if !attempted {
                return Ok(name);
            }
            let since = *settled_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= timing.settle {
                return Ok(name);
            }
            std::thread::sleep(timing.poll);
            continue;
        }
        if attempted {
            return Err(startup_failure(sub, &name, &obs, attempt_error));
        }
        if TERMINATED.contains(&status) || status == "created" {
            match sub.remove(&obs.id)? {
                RemoveOutcome::Removed => {}
                RemoveOutcome::StillRunning => std::thread::sleep(timing.poll),
            }
            continue;
        }
        return Err(format!("{name} is in unexpected state {status:?}"));
    }
}

fn create(sub: &dyn Substrate, desired: &Desired) -> Result<(), RunError> {
    let mut env = desired.env.clone();
    env.insert(START_NONCE_KEY.to_string(), new_generation());
    sub.run(&desired.spec, &env).map(|_| ())
}

fn startup_failure(
    sub: &dyn Substrate,
    name: &str,
    obs: &Observed,
    attempt_error: Option<String>,
) -> String {
    let logs = format!("{} logs {name}", sub.describe());
    match (obs.status.as_str(), attempt_error) {
        (_, Some(err)) => format!(
            "{err} (container left as {:?} for inspection: {logs})",
            obs.status
        ),
        ("created", None) => format!("{name} was created but never started; inspect with: {logs}"),
        (status, None) => format!(
            "the agent exited during startup ({status}, exit code {}); inspect with: {logs}. \
             The container is kept so the next Start can clear it.",
            obs.exit_code
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{LABEL_MANAGED_BY, LABEL_PUBKEY_FULL};
    use std::cell::RefCell;

    const NSEC: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";

    type RunHook = Box<dyn Fn(&RunSpec) -> Result<Observed, RunError>>;

    #[derive(Default)]
    struct Fake {
        container: RefCell<Option<Observed>>,
        runs: RefCell<Vec<BTreeMap<String, String>>>,
        removes: RefCell<u32>,
        on_run: RefCell<Option<RunHook>>,
        dies_after_inspects: RefCell<Option<u32>>,
    }

    impl Substrate for Fake {
        fn inspect(&self, _name: &str) -> Result<Option<Observed>, String> {
            let mut left = self.dies_after_inspects.borrow_mut();
            if let Some(n) = left.as_mut() {
                if *n == 0 {
                    if let Some(c) = self.container.borrow_mut().as_mut() {
                        c.status = "exited".into();
                        c.exit_code = 1;
                    }
                } else {
                    *n -= 1;
                }
            }
            Ok(self.container.borrow().clone())
        }
        fn run(&self, spec: &RunSpec, env: &BTreeMap<String, String>) -> Result<String, RunError> {
            self.runs.borrow_mut().push(env.clone());
            let made = match self.on_run.borrow().as_ref() {
                Some(f) => f(spec)?,
                None => Observed {
                    id: "new".into(),
                    status: "running".into(),
                    labels: spec.labels.clone(),
                    exit_code: 0,
                },
            };
            *self.container.borrow_mut() = Some(made);
            Ok("new".into())
        }
        fn remove(&self, _id: &str) -> Result<RemoveOutcome, String> {
            *self.removes.borrow_mut() += 1;
            let running = self
                .container
                .borrow()
                .as_ref()
                .is_some_and(|c| c.status == "running");
            if running {
                return Ok(RemoveOutcome::StillRunning);
            }
            *self.container.borrow_mut() = None;
            Ok(RemoveOutcome::Removed)
        }
        fn describe(&self) -> String {
            "docker".into()
        }
    }

    fn identity() -> AgentIdentity {
        AgentIdentity::from_nsec(NSEC).unwrap()
    }

    fn desired() -> Desired {
        let id = identity();
        Desired {
            spec: RunSpec {
                name: id.container_name(),
                image: "img:1".into(),
                labels: id.labels(),
                network: None,
                add_hosts: vec![],
                mounts: vec![],
                cpus: "2".into(),
                memory: "4g".into(),
                stop_timeout_secs: 60,
            },
            env: BTreeMap::from([("BUZZ_RELAY_URL".to_string(), "wss://r".to_string())]),
        }
    }

    fn fast() -> Timing {
        Timing {
            deadline: Duration::from_secs(5),
            poll: Duration::from_millis(1),
            settle: Duration::from_millis(5),
        }
    }

    fn ours(status: &str, exit_code: i64) -> Observed {
        Observed {
            id: "old".into(),
            status: status.into(),
            labels: identity().labels(),
            exit_code,
        }
    }

    #[test]
    fn no_container_creates_one_and_confirms_startup() {
        let fake = Fake::default();
        assert_eq!(
            deploy(&fake, &identity(), &desired(), &fast()).unwrap(),
            identity().container_name()
        );
        assert_eq!(fake.runs.borrow().len(), 1);
    }

    #[test]
    fn live_agent_is_a_strict_no_op() {
        let fake = Fake::default();
        *fake.container.borrow_mut() = Some(ours("running", 0));
        assert!(deploy(&fake, &identity(), &desired(), &fast()).is_ok());
        assert!(fake.runs.borrow().is_empty(), "created a second instance");
        assert_eq!(*fake.removes.borrow(), 0, "touched a live agent");
    }

    #[test]
    fn terminated_agent_is_cleared_and_restarted() {
        for status in ["exited", "dead", "created"] {
            let fake = Fake::default();
            *fake.container.borrow_mut() = Some(ours(status, 0));
            assert!(
                deploy(&fake, &identity(), &desired(), &fast()).is_ok(),
                "{status}"
            );
            assert_eq!(*fake.removes.borrow(), 1, "{status}");
            assert_eq!(fake.runs.borrow().len(), 1, "{status}");
        }
    }

    #[test]
    fn foreign_container_with_our_name_is_never_touched() {
        for labels in [
            BTreeMap::new(),
            BTreeMap::from([(
                LABEL_MANAGED_BY.to_string(),
                "buzz-backend-docker".to_string(),
            )]),
            {
                let mut l = identity().labels();
                l.insert(LABEL_PUBKEY_FULL.into(), "f".repeat(64));
                l
            },
        ] {
            let fake = Fake::default();
            *fake.container.borrow_mut() = Some(Observed {
                id: "x".into(),
                status: "exited".into(),
                labels,
                exit_code: 0,
            });
            let err = deploy(&fake, &identity(), &desired(), &fast()).unwrap_err();
            assert!(err.contains("not created by this provider"), "{err}");
            assert_eq!(*fake.removes.borrow(), 0);
            assert!(fake.runs.borrow().is_empty());
        }
    }

    #[test]
    fn startup_crash_is_reported_once_never_retried() {
        let fake = Fake::default();
        *fake.dies_after_inspects.borrow_mut() = Some(1);
        let err = deploy(&fake, &identity(), &desired(), &fast()).unwrap_err();
        assert!(err.contains("exit code 1"), "{err}");
        assert_eq!(fake.runs.borrow().len(), 1, "retried within one call");
        assert_eq!(
            *fake.removes.borrow(),
            0,
            "cleared the failed attempt within the same call"
        );
    }

    #[test]
    fn losing_a_create_race_adopts_the_winner() {
        let fake = Fake::default();
        let sub = Racing {
            inner: &fake,
            winner: ours("running", 0),
        };
        assert!(deploy(&sub, &identity(), &desired(), &fast()).is_ok());
        assert_eq!(*fake.removes.borrow(), 0, "deleted the winner");
    }

    struct Racing<'a> {
        inner: &'a Fake,
        winner: Observed,
    }

    impl Substrate for Racing<'_> {
        fn inspect(&self, name: &str) -> Result<Option<Observed>, String> {
            self.inner.inspect(name)
        }
        fn run(&self, _spec: &RunSpec, env: &BTreeMap<String, String>) -> Result<String, RunError> {
            *self.inner.container.borrow_mut() = Some(self.winner.clone());
            self.inner.runs.borrow_mut().push(env.clone());
            Err(RunError::Conflict)
        }
        fn remove(&self, id: &str) -> Result<RemoveOutcome, String> {
            self.inner.remove(id)
        }
        fn describe(&self) -> String {
            "docker".into()
        }
    }

    #[test]
    fn every_attempt_gets_a_fresh_start_nonce() {
        let fake = Fake::default();
        deploy(&fake, &identity(), &desired(), &fast()).unwrap();
        let nonce = &fake.runs.borrow()[0][START_NONCE_KEY];
        assert_eq!(nonce.len(), 8);
    }

    #[test]
    fn run_failure_surfaces_dockers_error() {
        let fake = Fake::default();
        *fake.on_run.borrow_mut() = Some(Box::new(|_| {
            Err(RunError::Failed(
                "docker run failed: pull access denied".into(),
            ))
        }));
        let err = deploy(&fake, &identity(), &desired(), &fast()).unwrap_err();
        assert!(err.contains("pull access denied"), "{err}");
    }
}
