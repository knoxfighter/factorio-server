use crate::credentials::CredentialsFailure;
use crate::instance::Status;
use async_zip::error::ZipError;
use std::num::ParseIntError;
use thiserror::Error;
use tokio::sync::broadcast::error::{RecvError, SendError};

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum ServerError {
    #[error("function not allowed for current Status: {0}")]
    NotAllowed(String),
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),
    #[error("rcon error: {0}")]
    Rcon(#[from] rcon::Error),
    #[error("tokio recv error: {0}")]
    TokioRecv(#[from] RecvError),
    #[error("utf-8 error")]
    Utf8Error(),
    #[error("send error: {0}")]
    TrackerSendError(#[from] SendError<String>),
    #[error("watch status channel send error: {0}")]
    WatchChannelSendError(#[from] tokio::sync::watch::error::SendError<Status>),
    #[error("watch status channel recv error: {0}")]
    WatchChannelRecvError(#[from] tokio::sync::watch::error::RecvError),
    #[error("parse int error: {0}")]
    ParseIntError(#[from] ParseIntError),
    #[error("ReqwestError: {0}")]
    ReqwestError(#[from] reqwest::Error),
    #[error("ZipError: {0}")]
    ZipError(#[from] ZipError),
    #[error("CredentialsFailure: {0}")]
    CredentialsFailure(#[from] CredentialsFailure),
    #[error("SerdeJsonError: {0}")]
    SerdeJsonError(#[from] serde_json::error::Error),
}
