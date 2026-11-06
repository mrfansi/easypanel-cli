use super::model::{DocId, Payload};
use eframe::egui;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::Instant,
};

pub(super) type RequestId = u64;
pub(super) type Outcome = Result<Payload, String>;
pub(super) type Sink = Arc<dyn Fn(crate::worker::Resp) + Send + Sync>;
type Run = Box<dyn FnOnce(Sink) -> anyhow::Result<Payload> + Send>;

#[derive(Clone)]
pub(crate) struct Runtime {
    pub make_cf_client: Arc<dyn Fn(&str) -> crate::cloudflare::CloudflareClient + Send + Sync>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            make_cf_client: Arc::new(crate::cloudflare::CloudflareClient::new),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Lane {
    Interactive,
    Poll,
    Transfer,
}

pub(super) struct TrustContext {
    pub target: String,
    pub secrets: Vec<String>,
}

struct Work {
    doc: DocId,
    request: RequestId,
    run: Run,
}
enum Event {
    Message(DocId, RequestId, crate::worker::Resp),
    Finished(DocId, RequestId, Outcome),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskStatus {
    Queued,
    Running,
    Complete,
    Partial,
    Failed,
}
pub(super) struct Task {
    pub id: RequestId,
    pub doc: DocId,
    pub label: String,
    pub target: String,
    pub progress: String,
    pub status: TaskStatus,
    pub started: Instant,
    pub polling: bool,
    secrets: Arc<[String]>,
}
struct Slot {
    request: RequestId,
    busy: bool,
    progress: String,
    result: Option<Outcome>,
    secrets: Arc<[String]>,
}

pub(super) struct Backend {
    interactive: SyncSender<Work>,
    poll: SyncSender<Work>,
    transfer: SyncSender<Work>,
    events: Receiver<Event>,
    slots: HashMap<(DocId, String), Slot>,
    pub tasks: VecDeque<Task>,
    next_request: RequestId,
    pub runtime: Runtime,
}

impl Backend {
    pub fn new(ctx: egui::Context, runtime: Runtime) -> Self {
        let (events_tx, events) = mpsc::sync_channel(256);
        let interactive = pool(4, 32, events_tx.clone(), ctx.clone());
        let poll = pool(3, 32, events_tx.clone(), ctx.clone());
        let transfer = pool(3, 24, events_tx, ctx);
        Self {
            interactive,
            poll,
            transfer,
            events,
            slots: HashMap::new(),
            tasks: VecDeque::new(),
            next_request: 1,
            runtime,
        }
    }

    pub fn submit(
        &mut self,
        doc: DocId,
        key: &str,
        label: &str,
        trust: TrustContext,
        lane: Lane,
        run: Run,
    ) {
        let slot_key = (doc, key.to_owned());
        if self.slots.get(&slot_key).is_some_and(|slot| slot.busy) {
            return;
        }
        let request = self.next_request;
        self.next_request += 1;
        let secrets: Arc<[String]> = trust.secrets.into();
        self.slots.insert(
            slot_key.clone(),
            Slot {
                request,
                busy: true,
                progress: "Queued".into(),
                result: None,
                secrets: secrets.clone(),
            },
        );
        let sender = match lane {
            Lane::Interactive => &self.interactive,
            Lane::Poll => &self.poll,
            Lane::Transfer => &self.transfer,
        };
        let result = sender.try_send(Work { doc, request, run });
        let (status, progress) = match result {
            Ok(()) => (TaskStatus::Queued, "Queued".to_string()),
            Err(TrySendError::Full(_)) => (
                TaskStatus::Failed,
                "The operation queue is full. Wait for a task to finish, then retry.".to_string(),
            ),
            Err(TrySendError::Disconnected(_)) => (
                TaskStatus::Failed,
                "The background worker stopped. Restart the application.".to_string(),
            ),
        };
        if status == TaskStatus::Failed {
            let slot = self.slots.get_mut(&slot_key).unwrap();
            slot.busy = false;
            slot.result = Some(Err(progress.clone()));
        }
        self.tasks.push_front(Task {
            id: request,
            doc,
            label: label.to_owned(),
            target: trust.target,
            progress,
            status,
            started: Instant::now(),
            polling: matches!(lane, Lane::Poll),
            secrets,
        });
        self.trim_tasks();
    }

    pub fn pump(&mut self) {
        for _ in 0..512 {
            let Ok(event) = self.events.try_recv() else {
                break;
            };
            let (doc, request) = match &event {
                Event::Message(doc, request, _) | Event::Finished(doc, request, _) => {
                    (*doc, *request)
                }
            };
            let task = self.tasks.iter_mut().find(|task| task.id == request);
            // A closed document discards payloads, not the task's final outcome.
            let slot = self
                .slots
                .iter_mut()
                .find(|((id, _), slot)| *id == doc && slot.request == request)
                .map(|(_, slot)| slot);
            let secrets = task
                .as_ref()
                .map(|task| task.secrets.clone())
                .or_else(|| slot.as_ref().map(|slot| slot.secrets.clone()))
                .unwrap_or_default();
            match event {
                Event::Message(_, _, crate::worker::Resp::Progress(message)) => {
                    let message = redact(&message, &secrets);
                    if let Some(task) = task {
                        if matches!(task.status, TaskStatus::Queued | TaskStatus::Running) {
                            task.progress = message.clone();
                            task.status = TaskStatus::Running;
                        }
                    }
                    if let Some(slot) = slot {
                        if slot.busy {
                            slot.progress = message;
                        }
                    }
                }
                Event::Message(_, _, response) => {
                    if let Some(message) = response_error(&response) {
                        let message = redact(message, &secrets);
                        if let Some(task) = task {
                            task.status = TaskStatus::Failed;
                            task.progress = message.clone();
                        }
                        if let Some(slot) = slot {
                            slot.result = Some(Err(message));
                        }
                    }
                }
                Event::Finished(_, _, result) => {
                    let result = sanitize(result, &secrets);
                    if let Some(task) = task {
                        // Terminal errors win over all subsequent progress/completion for this request.
                        if !matches!(task.status, TaskStatus::Failed | TaskStatus::Partial) {
                            let (status, message) = outcome_summary(&result);
                            task.status = status;
                            task.progress = redact(&message, &secrets);
                        }
                    }
                    if let Some(slot) = slot {
                        slot.busy = false;
                        if slot.result.is_none() {
                            slot.result = Some(result);
                        }
                    }
                }
            }
        }
        self.trim_tasks();
    }

    pub fn take(&mut self, doc: DocId, key: &str) -> Option<Outcome> {
        self.slots.get_mut(&(doc, key.to_owned()))?.result.take()
    }
    pub fn busy(&self, doc: DocId, key: &str) -> bool {
        self.slots
            .get(&(doc, key.to_owned()))
            .is_some_and(|slot| slot.busy)
    }
    pub fn progress(&self, doc: DocId, key: &str) -> Option<&str> {
        self.slots
            .get(&(doc, key.to_owned()))
            .map(|slot| slot.progress.as_str())
    }
    pub fn close(&mut self, doc: DocId) {
        self.slots.retain(|(id, _), _| *id != doc);
    }
    pub fn active(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| !t.polling && matches!(t.status, TaskStatus::Queued | TaskStatus::Running))
            .count()
    }
    fn trim_tasks(&mut self) {
        while self.tasks.len() > 300 {
            let Some(index) = self.tasks.iter().rposition(|task| {
                matches!(
                    task.status,
                    TaskStatus::Complete | TaskStatus::Partial | TaskStatus::Failed
                )
            }) else {
                break;
            };
            self.tasks.remove(index);
        }
    }
}

fn pool(
    count: usize,
    capacity: usize,
    events: SyncSender<Event>,
    ctx: egui::Context,
) -> SyncSender<Work> {
    let (tx, rx) = mpsc::sync_channel::<Work>(capacity);
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..count {
        let rx = rx.clone();
        let events = events.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || loop {
            let work = {
                match rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                }
            };
            let Ok(Work { doc, request, run }) = work else {
                return;
            };
            let event_tx = events.clone();
            let wake = ctx.clone();
            let sink: Sink = Arc::new(move |response| {
                let event = Event::Message(doc, request, response);
                // Progress is expendable under backpressure; failures are not.
                match event {
                    Event::Message(_, _, crate::worker::Resp::Progress(_)) => {
                        let _ = event_tx.try_send(event);
                    }
                    _ => {
                        let _ = event_tx.send(event);
                    }
                }
                wake.request_repaint();
            });
            sink(crate::worker::Resp::Progress("Running".into()));
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(sink)))
                .map_err(|_| {
                    "The operation stopped unexpectedly. Inspect the target before retrying."
                        .to_owned()
                })
                .and_then(|result| result.map_err(|error| error.to_string()));
            if events.send(Event::Finished(doc, request, result)).is_err() {
                return;
            }
            ctx.request_repaint();
        });
    }
    tx
}

fn response_error(response: &crate::worker::Resp) -> Option<&str> {
    use crate::worker::{CfResp, Resp};
    match response {
        Resp::Err(message)
        | Resp::StatsErr(message)
        | Resp::DomainsErr(message)
        | Resp::AllServicesErr(message)
        | Resp::DbmsFailed { message }
        | Resp::Branches(Err(message))
        | Resp::HostStat {
            data: Err(message), ..
        }
        | Resp::SshTested(Err(message))
        | Resp::Cf(CfResp::Err(message) | CfResp::WebAnalyticsErr(message)) => Some(message),
        _ => None,
    }
}
fn sanitize(result: Outcome, secrets: &[String]) -> Outcome {
    use crate::worker::{CfResp, Resp};
    let mut result = result.map_err(|message| redact(&message, secrets));
    if let Ok(Payload::Worker(response)) = &mut result {
        match response {
            Resp::Err(message)
            | Resp::StatsErr(message)
            | Resp::DomainsErr(message)
            | Resp::AllServicesErr(message)
            | Resp::DbmsFailed { message }
            | Resp::Branches(Err(message))
            | Resp::HostStat {
                data: Err(message), ..
            }
            | Resp::SshTested(Err(message))
            | Resp::Done(message, _)
            | Resp::Cf(
                CfResp::Err(message)
                | CfResp::WebAnalyticsErr(message)
                | CfResp::Done(message)
                | CfResp::Status(message),
            ) => *message = redact(message, secrets),
            Resp::BulkDone { failed, .. }
            | Resp::DomainsEdited { failed, .. }
            | Resp::DomainsDeleted { failed, .. }
            | Resp::Cf(CfResp::BulkDone { failed, .. }) => {
                for (_, message) in failed {
                    *message = redact(message, secrets);
                }
            }
            Resp::MaintInfo(rows) => {
                for (_, result) in rows {
                    if let Err(message) = result {
                        *message = redact(message, secrets);
                    }
                }
            }
            _ => {}
        }
    }
    result
}
fn outcome_summary(result: &Outcome) -> (TaskStatus, String) {
    use crate::worker::{CfResp, Resp};
    match result {
        Err(error) => (TaskStatus::Failed, error.clone()),
        Ok(Payload::ChosenPath(Some(_))) => (TaskStatus::Complete, "File selected".into()),
        Ok(Payload::ChosenPath(None)) => (TaskStatus::Complete, "Selection cancelled".into()),
        Ok(Payload::Worker(response)) => {
            if let Some(error) = response_error(response) {
                return (TaskStatus::Failed, error.into());
            }
            let bulk = match response {
                Resp::BulkDone { ok, failed, .. } => Some((ok.len(), failed)),
                Resp::DomainsEdited { ok, failed }
                | Resp::DomainsDeleted { ok, failed }
                | Resp::Cf(CfResp::BulkDone { ok, failed }) => Some((*ok, failed)),
                _ => None,
            };
            if let Some((ok, failed)) = bulk {
                let mut message = if matches!(response, Resp::BulkDone { action, .. } if action == "deploy")
                {
                    format!("{ok} deployment requests dispatched, {} refused. Builds may still be running; inspect service logs for deployment results.", failed.len())
                } else {
                    format!("{ok} completed, {} failed.", failed.len())
                };
                for (target, error) in failed {
                    message.push_str(&format!("\n{target}: {error}"));
                }
                return (
                    if failed.is_empty() {
                        TaskStatus::Complete
                    } else if ok == 0 {
                        TaskStatus::Failed
                    } else {
                        TaskStatus::Partial
                    },
                    message,
                );
            }
            match response {
                Resp::ServiceCreated {
                    project,
                    service,
                    stype,
                    notes,
                    pending,
                } => {
                    let mut message = format!("Created {project}/{service} ({stype}).");
                    let repair = pending.is_some() || !notes.is_empty();
                    if repair {
                        message.push_str(" Configuration needs attention; repair this service rather than creating it again.");
                        if let Some(phase) = pending {
                            message.push_str(match phase {
                                crate::worker::ServiceSetupPhase::Source => {
                                    " Source setup is pending."
                                }
                                crate::worker::ServiceSetupPhase::AutoDeploy => {
                                    " Source is saved; auto-deploy setup is pending."
                                }
                            });
                        }
                        for note in notes {
                            message.push('\n');
                            message.push_str(note);
                        }
                    }
                    (
                        if repair {
                            TaskStatus::Partial
                        } else {
                            TaskStatus::Complete
                        },
                        message,
                    )
                }
                Resp::MaintInfo(rows) => {
                    let failed = rows.iter().filter(|(_, result)| result.is_err()).count();
                    let mut message =
                        format!("{} completed, {failed} failed.", rows.len() - failed);
                    for (target, result) in rows {
                        if let Err(error) = result {
                            message.push_str(&format!("\n{target}: {error}"));
                        }
                    }
                    (
                        if failed == 0 {
                            TaskStatus::Complete
                        } else if failed == rows.len() {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Partial
                        },
                        message,
                    )
                }
                Resp::Done(message, _)
                | Resp::Cf(CfResp::Done(message) | CfResp::Status(message)) => {
                    (TaskStatus::Complete, message.clone())
                }
                Resp::Notes { msg, notes, .. } => (
                    TaskStatus::Complete,
                    format!("{msg}\\n{}", notes.join("\\n")),
                ),
                Resp::ProjectEnvSaved(_) => (
                    TaskStatus::Complete,
                    "Environment saved. Deploy services to apply the changed configuration.".into(),
                ),
                _ => (TaskStatus::Complete, "Completed".into()),
            }
        }
        _ => (TaskStatus::Complete, "Completed".into()),
    }
}
pub(super) fn redact(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_owned(), |text, secret| {
            text.replace(secret, "[redacted]")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn queued() -> (Backend, SyncSender<Event>) {
        let (work, _) = mpsc::sync_channel(1);
        let (events_tx, events) = mpsc::sync_channel(16);
        let mut backend = Backend {
            interactive: work.clone(),
            poll: work.clone(),
            transfer: work,
            events,
            slots: HashMap::new(),
            tasks: VecDeque::new(),
            next_request: 10,
            runtime: Runtime::default(),
        };
        let secrets: Arc<[String]> = vec!["private".into()].into();
        backend.slots.insert(
            (7, "fetch".into()),
            Slot {
                request: 9,
                busy: true,
                progress: String::new(),
                result: None,
                secrets: secrets.clone(),
            },
        );
        backend.tasks.push_back(Task {
            id: 9,
            doc: 7,
            label: "Transfer".into(),
            target: "host/db".into(),
            progress: String::new(),
            status: TaskStatus::Running,
            started: Instant::now(),
            polling: false,
            secrets,
        });
        (backend, events_tx)
    }
    #[test]
    fn submission_keeps_target_and_secret_redaction_together() {
        let (mut backend, tx) = queued();
        let (work_tx, work_rx) = mpsc::sync_channel(1);
        backend.interactive = work_tx;
        backend.submit(
            8,
            "load",
            "Load settings",
            TrustContext {
                target: "other-host/app".into(),
                secrets: vec!["other-secret".into()],
            },
            Lane::Interactive,
            Box::new(|_| Ok(Payload::Text(String::new()))),
        );
        let work = work_rx.try_recv().unwrap();
        assert_eq!(work.doc, 8);
        assert_eq!(backend.tasks[0].target, "other-host/app");
        assert_eq!(backend.tasks[0].label, "Load settings");
        assert!(backend.busy(8, "load"));
        assert!(tx
            .send(Event::Finished(
                8,
                work.request,
                Err("other-secret failed".into())
            ))
            .is_ok());
        backend.pump();
        assert_eq!(backend.tasks[0].progress, "[redacted] failed");
        assert!(matches!(
            backend.take(8, "load"),
            Some(Err(error)) if error == "[redacted] failed"
        ));
    }
    #[test]
    fn closed_document_keeps_redacted_late_failure_in_tasks() {
        let (mut backend, tx) = queued();
        backend.close(7);
        assert!(tx
            .send(Event::Finished(7, 9, Err("private failed".into())))
            .is_ok());
        backend.pump();
        assert!(backend.take(7, "fetch").is_none());
        assert!(matches!(backend.tasks[0].status, TaskStatus::Failed));
        assert_eq!(backend.tasks[0].progress, "[redacted] failed");
        assert_eq!(backend.active(), 0);
    }
    #[test]
    fn cancelled_file_selection_releases_slot_without_claiming_a_save() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(7, 9, Ok(Payload::ChosenPath(None))))
            .is_ok());
        backend.pump();
        assert!(!backend.busy(7, "fetch"));
        assert_eq!(backend.active(), 0);
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert_eq!(backend.tasks[0].progress, "Selection cancelled");
        assert!(matches!(
            backend.take(7, "fetch"),
            Some(Ok(Payload::ChosenPath(None)))
        ));
    }
    #[test]
    fn closed_document_drops_chosen_path_but_keeps_selection_outcome() {
        let (mut backend, tx) = queued();
        backend.close(7);
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::ChosenPath(Some("private/query.sql".into())))
            ))
            .is_ok());
        backend.pump();
        assert!(backend.take(7, "fetch").is_none());
        assert!(backend.slots.is_empty());
        assert_eq!(backend.active(), 0);
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert_eq!(backend.tasks[0].progress, "File selected");
    }
    #[test]
    fn asynchronous_failure_cannot_regress_to_running_or_success() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Message(
                7,
                9,
                crate::worker::Resp::Err("private failed".into())
            ))
            .is_ok());
        assert!(tx
            .send(Event::Message(
                7,
                9,
                crate::worker::Resp::Progress("still running".into())
            ))
            .is_ok());
        assert!(tx
            .send(Event::Finished(7, 9, Ok(Payload::Text("accepted".into()))))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Failed));
        assert_eq!(backend.tasks[0].progress, "[redacted] failed");
        assert!(matches!(backend.take(7, "fetch"), Some(Err(_))));
    }
    #[test]
    fn structured_failures_and_partial_bulk_keep_typed_page_payload() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Worker(crate::worker::Resp::Cf(
                    crate::worker::CfResp::BulkDone {
                        ok: 1,
                        failed: vec![("record".into(), "private refused".into())]
                    }
                )))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Partial));
        assert!(matches!(
            backend.take(7, "fetch"),
            Some(Ok(Payload::Worker(crate::worker::Resp::Cf(
                crate::worker::CfResp::BulkDone { .. }
            ))))
        ));
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Worker(crate::worker::Resp::StatsErr(
                    "private refused".into()
                )))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Failed));
        assert!(matches!(
            backend.take(7, "fetch"),
            Some(Ok(Payload::Worker(crate::worker::Resp::StatsErr(_))))
        ));
    }
    #[test]
    fn superseded_request_does_not_replace_current_result() {
        let (mut backend, tx) = queued();
        backend.slots.get_mut(&(7, "fetch".into())).unwrap().request = 10;
        assert!(tx
            .send(Event::Finished(7, 9, Ok(Payload::Text("old".into()))))
            .is_ok());
        backend.pump();
        assert!(backend.busy(7, "fetch"));
        assert!(backend.take(7, "fetch").is_none());
    }
    #[test]
    fn text_result_stays_private_to_document_and_never_enters_task_history() {
        let (mut backend, tx) = queued();
        let connector_token = "new-generated-connector-token";
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Text(connector_token.into()))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert_eq!(backend.tasks[0].progress, "Completed");
        assert!(!backend.tasks[0].progress.contains(connector_token));
        assert!(
            matches!(backend.take(7, "fetch"), Some(Ok(Payload::Text(value))) if value == connector_token)
        );
    }
    #[test]
    fn created_service_with_repair_notes_is_partial_not_creation_failure() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Worker(crate::worker::Resp::ServiceCreated {
                    project: "shop".into(),
                    service: "api".into(),
                    stype: "app".into(),
                    notes: vec!["Source configuration needs repair.".into()],
                    pending: Some(crate::worker::ServiceSetupPhase::Source),
                }))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Partial));
        assert!(backend.tasks[0]
            .progress
            .starts_with("Created shop/api (app)."));
        assert!(backend.tasks[0]
            .progress
            .contains("rather than creating it again"));
        assert!(matches!(
            backend.take(7, "fetch"),
            Some(Ok(Payload::Worker(
                crate::worker::Resp::ServiceCreated { .. }
            )))
        ));
    }
    #[test]
    fn bulk_deploy_completion_reports_dispatch_not_finished_builds() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Worker(crate::worker::Resp::BulkDone {
                    action: "deploy".into(),
                    ok: vec!["shop/api".into()],
                    failed: vec![],
                }))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert!(backend.tasks[0]
            .progress
            .contains("deployment requests dispatched"));
        assert!(backend.tasks[0]
            .progress
            .contains("Builds may still be running"));
        assert!(!backend.tasks[0].progress.contains("completed"));
    }
    #[test]
    fn completed_service_setup_reports_created_target() {
        let (mut backend, tx) = queued();
        assert!(tx
            .send(Event::Finished(
                7,
                9,
                Ok(Payload::Worker(crate::worker::Resp::ServiceCreated {
                    project: "shop".into(),
                    service: "api".into(),
                    stype: "app".into(),
                    notes: vec![],
                    pending: None,
                }))
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert_eq!(backend.tasks[0].progress, "Created shop/api (app).");
    }
}

#[cfg(test)]
mod late_event_tests {
    use super::*;
    #[test]
    fn late_error_after_completion_is_retained_and_progress_cannot_hide_it() {
        let (work, _) = mpsc::sync_channel(1);
        let (events_tx, events) = mpsc::sync_channel(8);
        let mut backend = Backend {
            interactive: work.clone(),
            poll: work.clone(),
            transfer: work,
            events,
            slots: HashMap::new(),
            tasks: VecDeque::new(),
            next_request: 2,
            runtime: Runtime::default(),
        };
        backend.slots.insert(
            (1, "deploy".into()),
            Slot {
                request: 1,
                busy: true,
                progress: String::new(),
                result: None,
                secrets: Arc::from([]),
            },
        );
        backend.tasks.push_back(Task {
            id: 1,
            doc: 1,
            label: "Deploy".into(),
            target: "host/project/service".into(),
            progress: String::new(),
            status: TaskStatus::Running,
            started: Instant::now(),
            polling: false,
            secrets: Arc::from([]),
        });
        assert!(events_tx
            .send(Event::Finished(1, 1, Ok(Payload::Text("Accepted".into()))))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Complete));
        assert!(backend.take(1, "deploy").is_some());
        assert!(events_tx
            .send(Event::Message(
                1,
                1,
                crate::worker::Resp::Err("Late deployment failure".into())
            ))
            .is_ok());
        assert!(events_tx
            .send(Event::Message(
                1,
                1,
                crate::worker::Resp::Progress("Old progress".into())
            ))
            .is_ok());
        backend.pump();
        assert!(matches!(backend.tasks[0].status, TaskStatus::Failed));
        assert_eq!(backend.tasks[0].progress, "Late deployment failure");
        assert!(matches!(backend.take(1, "deploy"), Some(Err(_))));
    }
}
