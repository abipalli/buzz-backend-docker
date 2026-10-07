//! Agent identity and every name derived from it (spec §Deploy State Machine
//! step 0, §Pod shape naming contract — label keys shared with
//! `buzz-backend-kubernetes`).

use std::collections::BTreeMap;

pub const MANAGED_BY: &str = "buzz-backend-docker";
pub const LABEL_MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const LABEL_BINDING_VERSION: &str = "buzz.block.xyz/binding-version";
pub const BINDING_VERSION: &str = "1";
pub const LABEL_AGENT_PUBKEY: &str = "buzz.block.xyz/agent-pubkey";
pub const LABEL_PUBKEY_FULL: &str = "buzz.block.xyz/agent-pubkey-full";
pub const LABEL_CREATE_INTENT: &str = "buzz.block.xyz/create-intent";
pub const LABEL_IMAGE: &str = "buzz.block.xyz/image";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentIdentity {
    pubkey_hex: String,
}

impl AgentIdentity {
    pub fn from_nsec(nsec: &str) -> Result<Self, String> {
        use nostr::FromBech32;
        let secret = nostr::SecretKey::from_bech32(nsec.trim())
            .map_err(|_| "private_key_nsec is not a decodable nsec1 key".to_string())?;
        Ok(Self {
            pubkey_hex: nostr::Keys::new(secret).public_key().to_hex(),
        })
    }

    pub fn pubkey_hex(&self) -> &str {
        &self.pubkey_hex
    }

    pub fn container_name(&self) -> String {
        format!("buzz-agent-{}", &self.pubkey_hex[..12])
    }

    pub fn labels(&self) -> BTreeMap<String, String> {
        [
            (LABEL_AGENT_PUBKEY, &self.pubkey_hex[..32]),
            (LABEL_PUBKEY_FULL, self.pubkey_hex.as_str()),
            (LABEL_MANAGED_BY, MANAGED_BY),
            (LABEL_BINDING_VERSION, BINDING_VERSION),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// True only for objects this provider created for this exact identity:
    /// the management marker *and* the full-pubkey label must both match.
    pub fn owns(&self, labels: &BTreeMap<String, String>) -> bool {
        labels.get(LABEL_MANAGED_BY).map(String::as_str) == Some(MANAGED_BY)
            && labels.get(LABEL_PUBKEY_FULL).map(String::as_str) == Some(self.pubkey_hex.as_str())
    }
}

pub fn new_generation() -> String {
    format!("{:08x}", rand::random::<u32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NSEC: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";

    #[test]
    fn derives_pubkey_and_names() {
        let id = AgentIdentity::from_nsec(NSEC).unwrap();
        assert_eq!(id.pubkey_hex().len(), 64);
        assert_eq!(id.container_name(), format!("buzz-agent-{}", &id.pubkey_hex()[..12]));
        assert_eq!(id.labels()[LABEL_AGENT_PUBKEY].len(), 32);
    }

    #[test]
    fn rejects_garbage_keys() {
        for bad in ["", "nsec1", "npub1abc", "not-a-key"] {
            assert!(AgentIdentity::from_nsec(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn ownership_needs_marker_and_full_pubkey() {
        let id = AgentIdentity::from_nsec(NSEC).unwrap();
        let mut labels = id.labels();
        assert!(id.owns(&labels));
        labels.insert(LABEL_MANAGED_BY.into(), "someone-else".into());
        assert!(!id.owns(&labels));
        let mut labels = id.labels();
        labels.insert(LABEL_PUBKEY_FULL.into(), "0".repeat(64));
        assert!(!id.owns(&labels));
    }
}
