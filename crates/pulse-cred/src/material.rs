use crate::resolve::Secret;
use anyhow::Result;
use rand::Rng;
use std::fs;
use std::path::Path;

/// Material is a private key on disk for the seconds of one push.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Material {
    pub dir: String,
    pub key_path: String,
    pub ssh_command: String,
}

impl crate::resolve::Resolver {
    /// Writes the key under PULSE_HOME/tmp with 0700 dir and 0600 file modes.
    pub fn materialize(&self, secret: &Secret) -> Result<Material> {
        let base = Path::new(&self.home).join("tmp");
        fs::create_dir_all(&base)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&base, fs::Permissions::from_mode(0o700))?;
        }
        let dir = loop {
            let mut buf = [0_u8; 8];
            rand::rng().fill_bytes(&mut buf);
            let dir = base.join(format!("push-{}", hex::encode(buf)));
            match fs::create_dir(&dir) {
                Ok(()) => break dir,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            }
        };
        let key_path = dir.join("key");
        if let Err(e) = fs::write(&key_path, &secret.key) {
            let _ = fs::remove_dir_all(&dir);
            return Err(e.into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
        }
        let key_path = key_path.display().to_string();
        Ok(Material {
            dir: dir.display().to_string(),
            ssh_command: ssh_command(&key_path),
            key_path,
        })
    }
}

/// SSHCommand is the GIT_SSH_COMMAND that pushes with exactly one key.
pub fn ssh_command(key_path: &str) -> String {
    format!(
        "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '{}'",
        key_path.replace('\'', r#"'\''"#)
    )
}

impl Material {
    /// Removes the key and its directory. Safe to call twice.
    pub fn cleanup(&self) {
        if self.dir.is_empty() {
            return;
        }
        if !self.key_path.is_empty()
            && let Ok(info) = fs::metadata(&self.key_path)
        {
            let _ = fs::write(&self.key_path, vec![0; info.len() as usize]);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{Resolver, Secret};

    const TEST_KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n";

    #[test]
    fn materialize_and_cleanup() {
        let _serial = crate::test_helpers::serial();
        let home = tempfile::tempdir().unwrap();
        let r = Resolver {
            home: home.path().display().to_string(),
            ..Resolver::default()
        };
        let m = r
            .materialize(&Secret {
                key: TEST_KEY.as_bytes().to_vec(),
                source: String::new(),
            })
            .unwrap();
        let info = fs::metadata(&m.key_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(info.permissions().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(&m.dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert!(
            m.dir.starts_with(&format!("{}/tmp/push-", r.home)) || m.dir.contains("\\tmp\\push-")
        );
        assert_eq!(
            m.ssh_command,
            format!(
                "ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '{}'",
                m.key_path
            )
        );
        m.cleanup();
        assert!(fs::metadata(&m.dir).is_err());
        m.cleanup();
        let none = Material::default();
        none.cleanup();
    }

    #[test]
    fn ssh_command_quotes_path() {
        let _serial = crate::test_helpers::serial();
        assert_eq!(
            ssh_command("/tmp/it's here/key"),
            r#"ssh -oBatchMode=yes -oIdentitiesOnly=yes -oIdentityAgent=none -i '/tmp/it'\''s here/key'"#
        );
    }
}
