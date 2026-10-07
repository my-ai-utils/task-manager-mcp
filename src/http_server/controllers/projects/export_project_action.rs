use std::path::PathBuf;
use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::project_transfer::ExportProjectInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

/// How much of the file is read off disk at a time, and how many of those chunks may be waiting to go out.
///
/// 64 KiB is a comfortable read and a frame size hyper does not have to split. Four of them in flight is the
/// backpressure: a reader on a slow connection makes the loop below block on `send` rather than pulling a
/// gigabyte off disk into a queue nobody is draining.
const CHUNK: usize = 64 * 1024;
const QUEUE: usize = 4;

#[http_route(
    method: "GET",
    route: "/api/projects/v1/export",
    controller: "Projects",
    summary: "Download a whole project as a zip",
    description: "Everything on the board, in one archive: `project.yaml` with the project's settings, `goals.yaml`, `tasks.yaml`, `comments.yaml`, `releases.yaml`, `documents.yaml`, and a `documents/` folder holding the project's documents as themselves, at their own paths. `documents.yaml` says what each of those files IS — above all the id it carries, which the folder cannot say, since a folder is keyed by path; that id is what every reference on every task and goal names a document by, so preserving it is what makes the references arrive working rather than empty. Prose — a task's text, a comment, a goal's description — travels base64-encoded inside the YAML so Markdown cannot be re-interpreted or re-indented; ids, statuses, labels, emails and moments stay legible. Deleted work is carried too, since leaving it out would make exporting a quiet way of losing the record; a connected repository is not, because it is a mirror of something that is still there. A GET rather than a POST so it can be an ordinary download link — the session is a cookie, which the browser attaches to a navigation. The archive is built into the temp directory as it is read out of the database and streamed back off disk, so nothing bigger than one document is ever held in memory.",
    input_data: "ExportProjectInputModel",
    result: [
        {status_code: 200, description: "The archive"},
        {status_code: 400, description: "No such project, or the archive could not be built"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
    ]
)]
pub struct ExportProjectAction {
    app: Arc<AppContext>,
}

impl ExportProjectAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ExportProjectAction,
    input_data: ExportProjectInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Membership rather than admin: an export is a read of a board, and it carries nothing somebody already
    // on that board cannot see. `require_project_by_prefix` resolves and checks in one call, and drops the
    // resolver's message — which names every prefix that exists, and must not be handed to somebody who is
    // not on this one.
    let project = crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let exported = crate::scripts::export_project(&action.app, &project.prefix)
        .await
        .map_err(bad_request)?;

    let (output, producer) = HttpOutput::as_stream(QUEUE);

    // Started before the response is returned, which is the point of the channel: the body is produced while
    // hyper is already writing it out, so the reader's first bytes do not wait for the last document.
    tokio::spawn(stream_and_remove(exported.path, producer));

    output
        // `attachment` and the name together are what make a browser save it rather than try to show it, and
        // they are the whole reason this is not `HttpOutput::as_file` — that one takes the bytes.
        .with_header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", exported.file_name),
        )
        .with_header("Content-Type", "application/zip")
        // Known before the first byte goes out, because the archive is already on disk — which is what gives
        // the reader a progress bar instead of a spinner.
        .with_header("Content-Length", exported.size.to_string())
        // The archive is a snapshot of a board that changes; a cached one would be yesterday's.
        .with_header("Cache-Control", "no-store")
        .get_result()
}

/// Send the file down the channel, then delete it.
///
/// **The removal is the whole reason this is a function rather than three lines inline.** It has to happen
/// on every way out — the download finishing, the reader closing the tab half way through it, a read error —
/// or the temp directory fills up with the exports of everybody who ever changed their mind. A `send` that
/// fails means the other end is gone, which is exactly the closed-tab case, so the loop stops on it rather
/// than reading the rest of a file nobody is receiving.
async fn stream_and_remove(path: PathBuf, mut producer: HttpOutputProducer) {
    use tokio::io::AsyncReadExt;

    match tokio::fs::File::open(&path).await {
        Ok(mut file) => {
            let mut buffer = vec![0u8; CHUNK];

            loop {
                match file.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(read) => {
                        if producer.send(buffer[..read].to_vec()).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        service_sdk::my_logger::LOGGER.write_error(
                            "ExportProject",
                            format!("reading the export back failed: {err}"),
                            service_sdk::my_logger::LogEventCtx::new(),
                        );
                        break;
                    }
                }
            }
        }
        Err(err) => {
            // Nothing can be sent, and the response headers have already gone out — so the reader gets a
            // truncated download. Logged rather than swallowed: it means the temp directory is not what this
            // process thinks it is, which is worth knowing about before every export fails.
            service_sdk::my_logger::LOGGER.write_error(
                "ExportProject",
                format!("the built export could not be opened: {err}"),
                service_sdk::my_logger::LogEventCtx::new(),
            );
        }
    }

    let _ = tokio::fs::remove_file(&path).await;
}
