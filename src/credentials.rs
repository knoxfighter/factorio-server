use crate::error::ServerError;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Formatter;
use std::fs::File;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub struct CredentialManager {
    save_file: PathBuf,
    credentials: Option<Credentials>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Credentials {
    pub username: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, Debug, Error)]
pub struct CredentialsFailure {
    error: String,
    message: String,
}

impl fmt::Display for CredentialsFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "error: {}, message: {}", self.error, self.message)
    }
}

impl CredentialManager {
    pub fn load(save_file: impl AsRef<Path>) -> Result<Self, ServerError> {
        let mut this = Self {
            save_file: save_file.as_ref().to_path_buf(),
            credentials: None,
        };
        if save_file.as_ref().exists() {
            let file = File::open(save_file)?;
            this.credentials = Some(serde_json::from_reader(file)?);
        }

        Ok(this)
    }

    // login based on https://wiki.factorio.com/Web_authentication_API
    pub async fn login(
        &mut self,
        username: impl AsRef<str>,
        password: impl AsRef<str>,
    ) -> Result<(), ServerError> {
        self.login_with_email_code(username, password, String::default())
            .await
    }

    pub async fn login_with_email_code(
        &mut self,
        username: impl AsRef<str>,
        password: impl AsRef<str>,
        email_code: impl AsRef<str>,
    ) -> Result<(), ServerError> {
        let client = reqwest::Client::new();

        let email_code = email_code.as_ref();

        let request = client.post("https://auth.factorio.com/api-login").form(&[
            ("username", username.as_ref()),
            ("password", password.as_ref()),
            ("api_version", "3"),
            ("require_game_ownership", "true"),
        ]);
        let request = if !email_code.is_empty() {
            request.form(&[("email_authentication_code", email_code)])
        } else {
            request
        };

        let resp = request.send().await?;

        println!("{}", resp.status());
        if resp.status().is_success() {
            self.credentials = Some(resp.json().await?);
            Ok(())
        } else {
            let failure: CredentialsFailure = resp.json().await?;
            Err(failure.into())
        }
    }

    pub fn login_with_token(&mut self, username: String, token: String) {
        self.credentials = Some(Credentials { username, token });
    }

    pub fn save(&self) -> Result<(), ServerError> {
        if self.credentials.is_some() {
            let file = File::create(&self.save_file)?;
            serde_json::to_writer(file, &self.credentials)?;
        } else {
            std::fs::remove_file(&self.save_file)?;
        }

        Ok(())
    }

    pub fn has_token(&self) -> bool {
        self.credentials.is_some()
    }

    pub fn get_credentials(&self) -> Result<Credentials, ServerError> {
        self.credentials
            .clone()
            .ok_or(ServerError::NotAllowed("No credentials found".to_string()))
    }

    pub fn logout(&mut self) {
        self.credentials = None;
    }
}
