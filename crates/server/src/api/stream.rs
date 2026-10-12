//! `GET /api/v1/displays/{id}/stream`: a WebSocket whose binary messages are frames.
//!
//! The stream owns the display until other content is set. Frames may arrive at any rate;
//! the display shows the newest one it can. Errors are reported as text messages with an
//! [`ErrorBody`] and do not close the stream. When the sender ends the stream, a schedule shows
//! what it says for the display again; without one, the last frame stays.
//!
//! With `zone=true`, the frames go into the `stream` zone of the layout the display shows
//! instead, until the display shows something else; the zone keeps the last frame.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use futures_util::SinkExt;
use ssp_core::Frame;

use super::types::{ErrorBody, StreamFormat, StreamQuery};
use super::{ApiError, AppState, blocking};
use crate::manager::StreamTicket;
use crate::sources::Feed;

/// Largest frame message accepted (raw RGBA for big panels).
const MAX_MESSAGE_BYTES: usize = 64 << 20;

/// Close code sent when other content replaced the stream (application-defined range).
const CLOSE_REPLACED: u16 = 4000;

/// How long a display keeps the last frame of a stream its sender ended before the schedule
/// takes over again, so that a sender that reconnects at once does not make it flicker.
const BACK_TO_SCHEDULE_AFTER: Duration = Duration::from_secs(2);

/// Where the frames of a stream go.
#[derive(Clone)]
enum Target {
    /// The whole display, which the stream owns.
    Display,
    /// The `stream` zone of the layout the display shows.
    Zone(Arc<Feed>),
}

/// Why a stream ended.
#[derive(PartialEq)]
enum End {
    /// The sender closed it or went away.
    Sender,
    /// Other content was set.
    Replaced,
    /// The daemon is stopping.
    Shutdown,
}

pub async fn stream(
    ws: WebSocketUpgrade,
    State(app): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<StreamQuery>,
) -> Result<Response, ApiError> {
    let manager = app.manager.clone();
    let zone = query.zone;
    let (ticket, target) = blocking(move || {
        if !zone {
            return Ok((manager.begin_stream(&id)?, Target::Display));
        }
        let (ticket, feed) = manager.stream_zone(&id)?.ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "the display shows no layout with a stream zone",
            )
        })?;
        Ok((ticket, Target::Zone(feed)))
    })
    .await?;
    tracing::info!(display = %ticket.id, zone, "stream started");
    Ok(ws
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, app, ticket, target, query)))
}

async fn run(
    mut socket: WebSocket,
    app: AppState,
    ticket: StreamTicket,
    target: Target,
    query: StreamQuery,
) {
    let mut shutdown = app.shutdown.clone();
    let fit = query.fit.unwrap_or_default().into();
    let end = loop {
        let message = tokio::select! {
            message = socket.recv() => message,
            _ = shutdown.changed() => break End::Shutdown,
        };
        let data = match message {
            Some(Ok(Message::Binary(data))) => data,
            Some(Ok(Message::Close(_)) | Err(_)) | None => break End::Sender,
            Some(Ok(_)) => continue,
        };
        if !ticket.is_current() {
            let close = CloseFrame {
                code: CLOSE_REPLACED,
                reason: "replaced by other content".into(),
            };
            let _ = socket.send(Message::Close(Some(close))).await;
            break End::Replaced;
        }
        let manager = app.manager.clone();
        let id = ticket.id.clone();
        let target = target.clone();
        let result = blocking(move || {
            let decode = |width, height| match query.format {
                StreamFormat::Image => Frame::decode(&data, width, height, fit),
                StreamFormat::Rgb => Frame::from_rgb(width, height, data.to_vec()),
                StreamFormat::Rgba => Frame::from_rgba(width, height, &data),
            };
            match target {
                Target::Display => {
                    // Looked up per frame so the stream survives the display reconnecting.
                    let device = manager.device(&id)?;
                    let panel = device.presenter.info().panel;
                    device.submit(decode(panel.width, panel.height)?)?;
                }
                Target::Zone(feed) => {
                    let (width, height) = feed.size().ok_or_else(|| {
                        ApiError::new(StatusCode::CONFLICT, "the layout is not shown yet")
                    })?;
                    let frame = decode(width, height).map_err(|e| {
                        ApiError::bad_request(format!("{e} (the zone is {width}x{height})"))
                    })?;
                    feed.put(frame);
                }
            }
            Ok(())
        })
        .await;
        if let Err(err) = result {
            let body = serde_json::to_string(&ErrorBody { error: err.message }).unwrap_or_default();
            if socket.send(Message::Text(body.into())).await.is_err() {
                break End::Sender;
            }
        }
    };
    // Sends the answer to the sender's close, which waits in the socket until flushed, and
    // finishes the closing handshake before waiting below.
    let _ = socket.flush().await;
    drop(socket);
    tracing::info!(display = %ticket.id, "stream ended");
    if end == End::Sender && matches!(target, Target::Display) {
        back_to_schedule(app, ticket).await;
    }
}

/// After the sender ended a stream, shows what the schedule says for the display, unless the
/// schedule is paused, the daemon is stopping, or other content (also a new stream) was set
/// meanwhile.
async fn back_to_schedule(app: AppState, ticket: StreamTicket) {
    let Some(schedule) = app.manager.schedule() else {
        return;
    };
    let mut shutdown = app.shutdown.clone();
    tokio::select! {
        () = tokio::time::sleep(BACK_TO_SCHEDULE_AFTER) => {}
        _ = shutdown.changed() => return,
    }
    if !ticket.is_current() || schedule.is_paused() {
        return;
    }
    tracing::info!(display = %ticket.id, "schedule: back after the stream");
    let manager = app.manager.clone();
    let _ = blocking(move || {
        schedule.apply_now_to(&manager, &ticket.id);
        Ok(())
    })
    .await;
}
