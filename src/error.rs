use std::num::ParseIntError;
use thiserror::Error;
use tokio::sync::broadcast::error::{RecvError, SendError};
use crate::instance::Status;

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
}