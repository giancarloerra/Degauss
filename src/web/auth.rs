use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SESSION_SECONDS: u64 = 12 * 60 * 60;
const REMEMBER_SECONDS: u64 = 30 * 24 * 60 * 60;
const MAX_SESSIONS: usize = 32;

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn random_token() -> Result<String, String> {
    let mut bytes = [0; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "The OS random generator failed.")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn digest(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
struct Password {
    hash: String,
    generation: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Session {
    pub csrf: String,
    pub expires: u64,
    pub remember: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct SavedSessions {
    generation: String,
    entries: BTreeMap<String, Session>,
}

pub(super) fn save(path: &Path, body: &str) -> Result<(), String> {
    const LABELS: crate::settings::SaveLabels = crate::settings::SaveLabels {
        creating_temporary: "creating temporary Web Access state",
        writing_temporary: "writing temporary Web Access state",
        flushing_temporary: "flushing temporary Web Access state",
        writing: "writing Web Access state",
        no_file_name: "Web Access state path has no filename",
        could_not_reserve: "could not reserve a temporary Web Access state file",
        installing: "installing Web Access state",
        flushing_directory: "flushing Web Access state directory",
    };
    match crate::settings::install_beside_settings(&LABELS, path, body)
        .map_err(|e| e.to_string())?
    {
        crate::settings::SaveOutcome::Durable => Ok(()),
        crate::settings::SaveOutcome::InstalledWithWarning(error) => Err(format!(
            "Web Access state was installed, but flushing its directory failed: {error}"
        )),
    }
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "Reading Web Access {} failed: {e}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "Web Access {} is malformed: {e}",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    })
}

pub(super) fn prepare_password(password: &str) -> Result<String, String> {
    if !(8..=128).contains(&password.chars().count()) {
        return Err("Use a password of 8 to 128 characters.".into());
    }
    let mut salt_bytes = [0; 16];
    SystemRandom::new()
        .fill(&mut salt_bytes)
        .map_err(|_| "The OS random generator failed.")?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| e.to_string())?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| format!("Hashing the web password failed: {e}"))?
        .to_string();
    let password = Password {
        hash,
        generation: random_token()?,
    };
    serde_json::to_string(&password).map_err(|e| e.to_string())
}

pub(super) fn install_password(directory: &Path, prepared: &str) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|e| format!("Creating Web Access state directory failed: {e}"))?;
    // The generation makes all old sessions invalid even if a later cleanup fails.
    save(&directory.join("password.json"), prepared)
}

pub(super) fn set_password(directory: &Path, password: &str) -> Result<(), String> {
    install_password(directory, &prepare_password(password)?)
}

pub(super) struct Auth {
    directory: PathBuf,
    password: Password,
    sessions: BTreeMap<String, Session>,
}

impl Auth {
    pub fn load(directory: PathBuf) -> Result<Self, String> {
        let password: Password = read(&directory.join("password.json"))?;
        PasswordHash::new(&password.hash)
            .map_err(|e| format!("The saved web password hash is malformed: {e}"))?;
        let path = directory.join("sessions.json");
        let saved = if path.exists() {
            read::<SavedSessions>(&path)?
        } else {
            SavedSessions::default()
        };
        let mut sessions = if saved.generation == password.generation {
            saved.entries
        } else {
            BTreeMap::new()
        };
        sessions.retain(|_, session| session.expires > now() && session.remember);
        Ok(Self {
            directory,
            password,
            sessions,
        })
    }

    pub fn credentials(&self) -> Credentials {
        Credentials {
            hash: self.password.hash.clone(),
            generation: self.password.generation.clone(),
        }
    }

    #[cfg(test)]
    pub fn login(&mut self, password: &str, remember: bool) -> Result<(String, Session), String> {
        let verified = self.credentials().verify(password)?;
        self.establish(verified, remember)
    }

    pub fn establish(
        &mut self,
        verified: Verified,
        remember: bool,
    ) -> Result<(String, Session), String> {
        if self.password.generation != verified.generation {
            return Err("The password changed during sign-in. Sign in again.".into());
        }
        self.sessions.retain(|_, session| session.expires > now());
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(
                "Too many active browser sessions. Sign out of another browser or change the web password to revoke all sessions.".into(),
            );
        }
        let token = random_token()?;
        let session = Session {
            csrf: random_token()?,
            expires: now()
                + if remember {
                    REMEMBER_SECONDS
                } else {
                    SESSION_SECONDS
                },
            remember,
        };
        self.sessions.insert(digest(&token), session.clone());
        if remember {
            if let Err(error) = self.persist() {
                self.sessions.remove(&digest(&token));
                return Err(error);
            }
        }
        Ok((token, session))
    }

    pub fn session(&self, token: &str) -> Option<Session> {
        if token.len() != 64 {
            return None;
        }
        self.sessions
            .get(&digest(token))
            .filter(|session| session.expires > now())
            .cloned()
    }

    pub fn logout(&mut self, token: &str) -> Result<(), String> {
        let key = digest(token);
        let old = self.sessions.remove(&key);
        if let Err(error) = self.persist() {
            if let Some(old) = old {
                self.sessions.insert(key, old);
            }
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        let saved = SavedSessions {
            generation: self.password.generation.clone(),
            entries: self
                .sessions
                .iter()
                .filter(|(_, v)| v.remember && v.expires > now())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        };
        save(
            &self.directory.join("sessions.json"),
            &serde_json::to_string(&saved).map_err(|e| e.to_string())?,
        )
    }
}

pub(super) struct Credentials {
    hash: String,
    generation: String,
}
pub(super) struct Verified {
    generation: String,
}

impl Credentials {
    pub fn verify(self, password: &str) -> Result<Verified, String> {
        if password.len() > 512 {
            return Err("Incorrect password.".into());
        }
        let parsed = PasswordHash::new(&self.hash)
            .map_err(|e| format!("The saved web password hash is malformed: {e}"))?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| "Incorrect password.")?;
        Ok(Verified {
            generation: self.generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_limit_counts_all_active_sessions_without_silently_revoking_browsers() {
        let directory =
            std::env::temp_dir().join(format!("degauss-web-limit-{}", random_token().unwrap()));
        set_password(&directory, "test-only-password").unwrap();
        let mut auth = Auth::load(directory.clone()).unwrap();
        let mut tokens = Vec::new();
        for at in 0..MAX_SESSIONS {
            let verified = Verified {
                generation: auth.password.generation.clone(),
            };
            tokens.push(auth.establish(verified, at % 2 == 0).unwrap().0);
        }
        for remember in [false, true] {
            let verified = Verified {
                generation: auth.password.generation.clone(),
            };
            let error = auth.establish(verified, remember).err().unwrap();
            assert!(error.contains("active browser sessions"));
            assert!(tokens.iter().all(|token| auth.session(token).is_some()));
        }
        auth.logout(&tokens[1]).unwrap();
        let verified = Verified {
            generation: auth.password.generation.clone(),
        };
        auth.establish(verified, false).unwrap();
        assert_eq!(auth.sessions.len(), MAX_SESSIONS);
        assert!(auth.session(&tokens[0]).is_some());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remembered_sessions_survive_restart_but_not_password_change_or_logout() {
        let directory =
            std::env::temp_dir().join(format!("degauss-web-auth-{}", random_token().unwrap()));
        set_password(&directory, "test-only-password").unwrap();
        let mut auth = Auth::load(directory.clone()).unwrap();
        assert!(auth.login("incorrect", true).is_err());
        let (token, session) = auth.login("test-only-password", true).unwrap();
        let (temporary, _) = auth.login("test-only-password", false).unwrap();
        let disk = std::fs::read_to_string(directory.join("sessions.json")).unwrap();
        assert!(!disk.contains(&token));
        assert!(!disk.contains("test-only-password"));
        let mut restored = Auth::load(directory.clone()).unwrap();
        assert_eq!(restored.session(&token).unwrap().csrf, session.csrf);
        assert!(restored.session(&temporary).is_none());
        restored.logout(&token).unwrap();
        assert!(Auth::load(directory.clone())
            .unwrap()
            .session(&token)
            .is_none());
        let (token, _) = restored.login("test-only-password", true).unwrap();
        restored.sessions.get_mut(&digest(&token)).unwrap().expires = now() - 1;
        assert!(restored.session(&token).is_none());
        restored.persist().unwrap();
        assert!(Auth::load(directory.clone())
            .unwrap()
            .session(&token)
            .is_none());
        let (token, _) = restored.login("test-only-password", true).unwrap();
        set_password(&directory, "different-test-password").unwrap();
        assert!(Auth::load(directory.clone())
            .unwrap()
            .session(&token)
            .is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
