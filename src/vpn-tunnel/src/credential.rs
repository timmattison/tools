use op_cache::ItemField;

/// A running gluetun container and its WireGuard key.
#[derive(Debug, Clone)]
pub struct RunningTunnel {
    pub container_name: String,
    pub wireguard_key: String,
}

/// The result of credential selection.
#[derive(Debug, Clone)]
pub struct SelectedCredential {
    /// The field label (e.g., "credential-2")
    pub field_label: String,
    /// The WireGuard private key value
    pub key: String,
    /// Total number of available credentials
    pub total: usize,
    /// Number of credentials currently in use
    pub in_use: usize,
}

/// A credential that is currently in use by a running container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialInUse {
    pub field_label: String,
    pub container_name: String,
}

/// Error when all credentials are in use.
#[derive(Debug)]
pub struct AllCredentialsInUse {
    pub usage: Vec<CredentialInUse>,
}

/// Selects the first unused credential from available fields.
///
/// Compares available credential values against keys used by running tunnels.
/// Returns the first credential whose value does not appear in any running tunnel.
///
/// # Errors
///
/// Returns `AllCredentialsInUse` if every credential is used by a running container.
pub fn select_credential(
    available: &[ItemField],
    running: &[RunningTunnel],
) -> Result<SelectedCredential, AllCredentialsInUse> {
    let mut first_free: Option<&ItemField> = None;
    let mut in_use_count = 0usize;

    for field in available {
        if running.iter().any(|r| r.wireguard_key == field.value) {
            in_use_count += 1;
        } else if first_free.is_none() {
            first_free = Some(field);
        }
    }

    if let Some(field) = first_free {
        return Ok(SelectedCredential {
            field_label: field.label.clone(),
            key: field.value.clone(),
            total: available.len(),
            in_use: in_use_count,
        });
    }

    // All in use — build the usage list
    let usage: Vec<CredentialInUse> = available
        .iter()
        .map(|field| {
            let container = running
                .iter()
                .find(|r| r.wireguard_key == field.value)
                .map(|r| r.container_name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            CredentialInUse {
                field_label: field.label.clone(),
                container_name: container,
            }
        })
        .collect();

    Err(AllCredentialsInUse { usage })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: &str, value: &str) -> ItemField {
        ItemField {
            label: label.to_string(),
            value: value.to_string(),
        }
    }

    fn tunnel(name: &str, key: &str) -> RunningTunnel {
        RunningTunnel {
            container_name: name.to_string(),
            wireguard_key: key.to_string(),
        }
    }

    #[test]
    fn single_credential_none_in_use() {
        let available = vec![field("credential", "key-1")];
        let running = vec![];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 1);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_none_in_use_selects_first() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.key, "key-1");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 0);
    }

    #[test]
    fn multiple_credentials_first_in_use_selects_second() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![tunnel("scraper-gluetun", "key-1")];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential-2");
        assert_eq!(result.key, "key-2");
        assert_eq!(result.total, 2);
        assert_eq!(result.in_use, 1);
    }

    #[test]
    fn all_credentials_in_use_returns_error() {
        let available = vec![
            field("credential", "key-1"),
            field("credential-2", "key-2"),
        ];
        let running = vec![
            tunnel("scraper-gluetun", "key-1"),
            tunnel("vpn-gluetun", "key-2"),
        ];
        let err = select_credential(&available, &running).unwrap_err();
        assert_eq!(err.usage.len(), 2);
        assert_eq!(
            err.usage[0],
            CredentialInUse {
                field_label: "credential".to_string(),
                container_name: "scraper-gluetun".to_string(),
            }
        );
        assert_eq!(
            err.usage[1],
            CredentialInUse {
                field_label: "credential-2".to_string(),
                container_name: "vpn-gluetun".to_string(),
            }
        );
    }

    #[test]
    fn running_tunnel_with_unknown_key_does_not_block() {
        let available = vec![field("credential", "key-1")];
        let running = vec![tunnel("other-gluetun", "different-key")];
        let result = select_credential(&available, &running).unwrap();
        assert_eq!(result.field_label, "credential");
        assert_eq!(result.in_use, 0);
    }
}
