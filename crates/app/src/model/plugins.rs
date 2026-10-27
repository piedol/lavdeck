//! The plugin host's state: installed plugins, their switches and settings, and jobs. The
//! bookkeeping (queueing per plugin, failures, capture toggles) is here and pure; running a
//! job is the backend's ([`crate::backend::Backend::launch`]).

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::takes::Take;

/// A JSON value for plugin settings.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonValue {
    String(String),
    Number(f64),
    Bool(bool),
    Null,
}

impl JsonValue {
    /// How a setting's value reads in a text field: `6` for 6.0, empty for null.
    pub fn text(&self) -> String {
        match self {
            JsonValue::String(s) => s.clone(),
            JsonValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            JsonValue::Number(n) => n.to_string(),
            JsonValue::Bool(b) => b.to_string(),
            JsonValue::Null => String::new(),
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingKind {
    Bool,
    String,
    Number,
    Choice,
    Path,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginSetting {
    pub key: String,
    pub title: String,
    pub kind: SettingKind,
    pub default: JsonValue,
    /// `(value, title)` for a choice.
    pub options: Vec<(String, String)>,
    pub help: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PluginAction {
    pub id: String,
    pub title: String,
    /// What a take needs for the action to be offered: `video`, `screen`, `camera`, `transcript`.
    pub requires: Vec<String>,
}

impl PluginAction {
    pub fn applies(&self, take: &Take) -> bool {
        self.requires.iter().all(|r| take.has(r))
    }
}

/// A companion app a plugin ships, opened on a take.
#[derive(Clone, Debug, PartialEq)]
pub struct AppSpec {
    pub bundle: String,
    pub open_take: Option<String>,
    pub action_title: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: Option<String>,
    pub events: Vec<String>,
    pub actions: Vec<PluginAction>,
    pub settings: Vec<PluginSetting>,
    pub produces: Vec<String>,
    /// `post` (default), `capture` or `app`.
    pub kind: Option<String>,
    pub permissions: Vec<String>,
    /// Sources a capture plugin records (`capture.sources`).
    pub capture_sources: Option<Vec<String>>,
    pub app: Option<AppSpec>,
}

impl Manifest {
    pub fn is_capture(&self) -> bool {
        self.kind.as_deref() == Some("capture") && self.capture_sources.is_some()
    }

    pub fn is_app(&self) -> bool {
        self.kind.as_deref() == Some("app") && self.app.is_some()
    }

    pub fn produces(&self, kind: &str) -> bool {
        self.produces.iter().any(|p| p == kind)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plugin {
    pub folder: PathBuf,
    pub manifest: Manifest,
}

impl Plugin {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

/// A source the deck can switch: one of a capture plugin's `capture.sources` that has a bool
/// setting with the same key.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureToggle {
    pub plugin_id: String,
    pub source: String,
}

impl CaptureToggle {
    pub fn id(&self) -> String {
        format!("{}/{}", self.plugin_id, self.source)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    /// Waits for the same plugin's running job: a plugin runs one job at a time.
    Queued,
    Running,
    Finished(Option<String>),
    Failed(String),
}

pub const CANCELLED: &str = "Cancelled";

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub plugin_id: String,
    pub plugin_name: String,
    pub take_id: String,
    pub title: String,
    /// Started by a finished take rather than by hand in Recordings.
    pub automatic: bool,
    pub fraction: f64,
    pub message: Option<String>,
    pub state: JobState,
    /// The user closed this job's failure note.
    pub dismissed: bool,
}

impl Job {
    pub fn failure(&self) -> Option<&str> {
        match &self.state {
            JobState::Failed(r) if r != CANCELLED => Some(r),
            _ => None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, JobState::Running | JobState::Queued)
    }
}

/// What to run for a job: a plugin request or Lavdeck's own work (making a take's video).
#[derive(Clone, Debug, PartialEq)]
pub enum Work {
    /// `kind` is `event` or `action`; `name` the event name or action id.
    Plugin {
        plugin_id: String,
        kind: String,
        name: String,
        take_id: String,
    },
    MakeVideo {
        take_id: String,
    },
}

/// A job the backend should start now.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub job_id: u64,
    pub work: Work,
}

/// Lavdeck's own video job, shown alongside plugin jobs.
pub const VIDEO_JOB_ID: &str = "lavdeck.video";
/// The take event plugins listen to.
pub const TAKE_FINISHED: &str = "take.finished";

#[derive(Clone, Debug, Default)]
pub struct PluginHost {
    pub plugins: Vec<Plugin>,
    pub problems: Vec<String>,
    pub jobs: Vec<Job>,
    /// `plugins.enabled` in the defaults domain.
    pub enabled: BTreeMap<String, bool>,
    /// `plugins.settings.<id>`: only values the user set.
    pub stored: BTreeMap<String, BTreeMap<String, JsonValue>>,
    /// Bumped whenever a job writes files or ends, so take lists refresh.
    pub outputs_revision: u64,
    /// Queued work by job id, started in order as the same plugin's job finishes.
    queued: BTreeMap<u64, Work>,
    next_id: u64,
}

impl PluginHost {
    pub fn plugin(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.id() == id)
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled.get(id).copied().unwrap_or(false)
    }

    pub fn enabled_plugins(&self) -> impl Iterator<Item = &Plugin> {
        self.plugins.iter().filter(|p| self.is_enabled(p.id()))
    }

    /// Enabled plugins that ship a companion app.
    pub fn app_plugins(&self) -> Vec<&Plugin> {
        self.enabled_plugins()
            .filter(|p| p.manifest.is_app())
            .collect()
    }

    /// Effective settings: the manifest defaults overlaid with the user's values.
    pub fn settings(&self, plugin: &Plugin) -> BTreeMap<String, JsonValue> {
        let mut out: BTreeMap<String, JsonValue> = plugin
            .manifest
            .settings
            .iter()
            .map(|s| (s.key.clone(), s.default.clone()))
            .collect();
        if let Some(stored) = self.stored.get(plugin.id()) {
            for (k, v) in stored {
                out.insert(k.clone(), v.clone());
            }
        }
        out
    }

    pub fn value(&self, plugin: &Plugin, key: &str) -> JsonValue {
        self.settings(plugin).remove(key).unwrap_or(JsonValue::Null)
    }

    pub fn set_value(&mut self, plugin_id: &str, key: &str, value: JsonValue) {
        let entry = self.stored.entry(plugin_id.to_string()).or_default();
        if value == JsonValue::Null {
            entry.remove(key);
        } else {
            entry.insert(key.to_string(), value);
        }
    }

    pub fn set_enabled(&mut self, plugin_id: &str, on: bool) {
        self.enabled.insert(plugin_id.to_string(), on);
    }

    /// Toggles for every installed capture plugin, enabled or not, so one click can switch a
    /// plugin on.
    pub fn capture_toggles(&self) -> Vec<CaptureToggle> {
        let mut out = Vec::new();
        for p in self.plugins.iter().filter(|p| p.manifest.is_capture()) {
            let bools: Vec<&str> = p
                .manifest
                .settings
                .iter()
                .filter(|s| s.kind == SettingKind::Bool)
                .map(|s| s.key.as_str())
                .collect();
            for source in p.manifest.capture_sources.iter().flatten() {
                if bools.contains(&source.as_str()) {
                    out.push(CaptureToggle {
                        plugin_id: p.id().to_string(),
                        source: source.clone(),
                    });
                }
            }
        }
        out
    }

    /// On when the plugin is enabled and its setting for this source is on.
    pub fn is_on(&self, toggle: &CaptureToggle) -> bool {
        self.is_enabled(&toggle.plugin_id)
            && self
                .plugin(&toggle.plugin_id)
                .is_some_and(|p| self.value(p, &toggle.source).as_bool() == Some(true))
    }

    pub fn set_on(&mut self, toggle: &CaptureToggle, on: bool) {
        if on && !self.is_enabled(&toggle.plugin_id) {
            // Switching a plugin on from the deck records just the source that was clicked.
            for other in self.capture_toggles() {
                if other.plugin_id == toggle.plugin_id {
                    self.set_value(&other.plugin_id, &other.source, JsonValue::Bool(false));
                }
            }
            self.set_enabled(&toggle.plugin_id, true);
        }
        self.set_value(&toggle.plugin_id, &toggle.source, JsonValue::Bool(on));
    }

    /// Enabled actions, paired with their plugin.
    pub fn actions(&self) -> Vec<(&Plugin, &PluginAction)> {
        self.enabled_plugins()
            .flat_map(|p| p.manifest.actions.iter().map(move |a| (p, a)))
            .collect()
    }

    pub fn actions_for(&self, take: &Take) -> Vec<(&Plugin, &PluginAction)> {
        self.actions()
            .into_iter()
            .filter(|(_, a)| a.applies(take))
            .collect()
    }

    /// The first action from a plugin that produces transcripts.
    pub fn transcribe_action(&self, take: Option<&Take>) -> Option<(&Plugin, &PluginAction)> {
        let all = match take {
            Some(t) => self.actions_for(t),
            None => self.actions(),
        };
        all.into_iter()
            .find(|(p, _)| p.manifest.produces("transcript"))
    }

    pub fn running_jobs(&self) -> Vec<&Job> {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Running)
            .collect()
    }

    pub fn queued_count(&self, plugin_id: &str) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Queued && j.plugin_id == plugin_id)
            .count()
    }

    /// The take's running or queued job, if any (optionally one plugin's).
    pub fn job(&self, take_id: &str, plugin: Option<&str>) -> Option<&Job> {
        self.jobs.iter().rev().find(|j| {
            j.take_id == take_id && j.is_active() && plugin.is_none_or(|p| j.plugin_id == p)
        })
    }

    /// The newest job per plugin among `jobs`, so an older failure doesn't outlive a newer run.
    fn newest_failure<'a>(
        jobs: impl DoubleEndedIterator<Item = &'a Job> + Clone,
    ) -> Option<&'a Job> {
        let mut newest: BTreeMap<&str, u64> = BTreeMap::new();
        for j in jobs.clone() {
            newest.insert(&j.plugin_id, j.id);
        }
        jobs.rev().find(|j| {
            newest.get(j.plugin_id.as_str()) == Some(&j.id) && j.failure().is_some() && !j.dismissed
        })
    }

    /// A failure among each plugin's newest automatic job, until dismissed or that plugin's next
    /// automatic job runs. Failures of actions started by hand show only where they started.
    pub fn automatic_failure(&self) -> Option<&Job> {
        Self::newest_failure(self.jobs.iter().filter(|j| j.automatic))
    }

    /// The take's undismissed failure (optionally one plugin's), unless a newer run followed it.
    pub fn failure(&self, take_id: &str, plugin: Option<&str>) -> Option<&Job> {
        Self::newest_failure(
            self.jobs
                .iter()
                .filter(move |j| j.take_id == take_id && plugin.is_none_or(|p| j.plugin_id == p)),
        )
    }

    pub fn dismiss(&mut self, job_id: u64) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            j.dismissed = true;
        }
    }

    /// Runs every enabled plugin that listens to `take.finished`.
    pub fn take_finished(&mut self, take: &Take) -> Vec<Launch> {
        let listeners: Vec<(String, String)> = self
            .enabled_plugins()
            .filter(|p| p.manifest.events.iter().any(|e| e == TAKE_FINISHED))
            .map(|p| (p.id().to_string(), p.manifest.name.clone()))
            .collect();
        listeners
            .into_iter()
            .filter_map(|(id, name)| {
                let work = Work::Plugin {
                    plugin_id: id.clone(),
                    kind: "event".into(),
                    name: TAKE_FINISHED.into(),
                    take_id: take.id.clone(),
                };
                self.start(&id, &name.clone(), &name, &take.id, true, work)
            })
            .collect()
    }

    /// Runs an action on a take, unless that plugin already has a job for it.
    pub fn run(&mut self, plugin_id: &str, action_id: &str, take_id: &str) -> Option<Launch> {
        if self.job(take_id, Some(plugin_id)).is_some() {
            return None;
        }
        let plugin = self.plugin(plugin_id)?;
        let name = plugin.manifest.name.clone();
        let title = plugin
            .manifest
            .actions
            .iter()
            .find(|a| a.id == action_id)?
            .title
            .clone();
        let work = Work::Plugin {
            plugin_id: plugin_id.to_string(),
            kind: "action".into(),
            name: action_id.to_string(),
            take_id: take_id.to_string(),
        };
        self.start(plugin_id, &name, &title, take_id, false, work)
    }

    /// Lavdeck's own work on a take as a job (`id` stands in for a plugin's).
    pub fn run_built_in(
        &mut self,
        id: &str,
        name: &str,
        title: &str,
        take_id: &str,
        automatic: bool,
        work: Work,
    ) -> Option<Launch> {
        if self.job(take_id, Some(id)).is_some() {
            return None;
        }
        self.start(id, name, title, take_id, automatic, work)
    }

    fn start(
        &mut self,
        plugin_id: &str,
        plugin_name: &str,
        title: &str,
        take_id: &str,
        automatic: bool,
        work: Work,
    ) -> Option<Launch> {
        self.next_id += 1;
        let id = self.next_id;
        let busy = self
            .jobs
            .iter()
            .any(|j| j.plugin_id == plugin_id && j.is_active());
        self.jobs.push(Job {
            id,
            plugin_id: plugin_id.to_string(),
            plugin_name: plugin_name.to_string(),
            take_id: take_id.to_string(),
            title: title.to_string(),
            automatic,
            fraction: 0.0,
            // A queued job says "Queued" in the views, from its state.
            message: None,
            state: if busy {
                JobState::Queued
            } else {
                JobState::Running
            },
            dismissed: false,
        });
        if busy {
            self.queued.insert(id, work);
            None
        } else {
            Some(Launch { job_id: id, work })
        }
    }

    /// A queued job is dropped at once; a running one is the backend's to stop (it reports
    /// back through [`PluginHost::finish`]). Returns whether the backend must cancel it.
    pub fn cancel(&mut self, job_id: u64) -> bool {
        if self.queued.remove(&job_id).is_some() {
            if let Some(j) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                j.state = JobState::Failed(CANCELLED.into());
            }
            return false;
        }
        self.jobs
            .iter()
            .any(|j| j.id == job_id && j.state == JobState::Running)
    }

    /// Jobs to cancel for these takes (before they go to the Trash): the running ones the
    /// backend must stop. Queued ones are dropped here.
    pub fn cancel_jobs_for(&mut self, take_ids: &[String]) -> Vec<u64> {
        let ids: Vec<u64> = self
            .jobs
            .iter()
            .filter(|j| take_ids.contains(&j.take_id) && j.is_active())
            .map(|j| j.id)
            .collect();
        ids.into_iter().filter(|id| self.cancel(*id)).collect()
    }

    pub fn progress(&mut self, job_id: u64, fraction: Option<f64>, message: Option<String>) {
        if let Some(j) = self
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id && j.state == JobState::Running)
        {
            if let Some(f) = fraction {
                j.fraction = f;
            }
            if message.is_some() {
                j.message = message;
            }
        }
    }

    /// Ends a job and starts the same plugin's next queued one, if any.
    pub fn finish(&mut self, job_id: u64, state: JobState) -> Option<Launch> {
        let i = self.jobs.iter().position(|j| j.id == job_id)?;
        if let JobState::Finished(note) = &state {
            self.jobs[i].fraction = 1.0;
            if self.jobs[i].message.is_none() {
                self.jobs[i].message = note.clone();
            }
        }
        self.jobs[i].state = state;
        self.outputs_revision += 1;
        let plugin = self.jobs[i].plugin_id.clone();
        let next = self
            .jobs
            .iter()
            .find(|j| j.plugin_id == plugin && j.state == JobState::Queued)?
            .id;
        let work = self.queued.remove(&next)?;
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == next) {
            j.state = JobState::Running;
            j.message = None;
        }
        Some(Launch { job_id: next, work })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> Manifest {
        Manifest {
            id: id.into(),
            name: id.to_uppercase(),
            version: "1".into(),
            summary: None,
            events: vec![TAKE_FINISHED.into()],
            actions: vec![PluginAction {
                id: "go".into(),
                title: "Go".into(),
                requires: vec![],
            }],
            settings: vec![],
            produces: vec!["transcript".into()],
            kind: None,
            permissions: vec![],
            capture_sources: None,
            app: None,
        }
    }

    fn host() -> PluginHost {
        let mut h = PluginHost::default();
        for id in ["a", "b"] {
            h.plugins.push(Plugin {
                folder: PathBuf::new(),
                manifest: manifest(id),
            });
            h.set_enabled(id, true);
        }
        h
    }

    #[test]
    fn one_job_per_plugin_at_a_time() {
        let mut h = host();
        let first = h.run("a", "go", "t1").expect("starts");
        assert!(h.run("a", "go", "t1").is_none(), "same take twice");
        assert!(h.run("a", "go", "t2").is_none(), "queued behind t1");
        assert_eq!(h.job("t2", Some("a")).unwrap().state, JobState::Queued);
        assert!(
            h.run("b", "go", "t2").is_some(),
            "another plugin runs at once"
        );
        let next = h
            .finish(first.job_id, JobState::Finished(None))
            .expect("t2 starts");
        assert_eq!(
            next.work,
            Work::Plugin {
                plugin_id: "a".into(),
                kind: "action".into(),
                name: "go".into(),
                take_id: "t2".into()
            }
        );
        assert_eq!(h.job("t2", Some("a")).unwrap().state, JobState::Running);
    }

    #[test]
    fn cancelling_a_queued_job_drops_it() {
        let mut h = host();
        h.run("a", "go", "t1");
        h.run("a", "go", "t2");
        let queued = h.job("t2", Some("a")).unwrap().id;
        assert!(!h.cancel(queued), "nothing for the backend to stop");
        assert!(h.job("t2", None).is_none());
        assert!(h.failure("t2", None).is_none(), "a cancel isn't a failure");
    }

    #[test]
    fn failures_clear_when_a_newer_run_follows() {
        let mut h = host();
        let job = h.run("a", "go", "t1").unwrap();
        h.finish(job.job_id, JobState::Failed("boom".into()));
        assert_eq!(h.failure("t1", Some("a")).unwrap().failure(), Some("boom"));
        assert!(h.automatic_failure().is_none(), "started by hand");
        let again = h.run("a", "go", "t1").unwrap();
        assert!(h.failure("t1", None).is_none());
        h.finish(again.job_id, JobState::Finished(Some("1 file(s)".into())));
        assert_eq!(h.jobs.last().unwrap().message.as_deref(), Some("1 file(s)"));
    }

    #[test]
    fn automatic_failures_are_per_plugin() {
        let mut h = host();
        let take = Take {
            id: "t1".into(),
            folder: PathBuf::new(),
            stem: "x".into(),
            started: None,
            duration: None,
            channel_mode: None,
            files: vec![],
            speakers: vec![],
        };
        let launches = h.take_finished(&take);
        assert_eq!(launches.len(), 2);
        h.finish(launches[0].job_id, JobState::Failed("no model".into()));
        h.finish(launches[1].job_id, JobState::Finished(None));
        let failed = h.automatic_failure().expect("a's failure still shows");
        assert_eq!(failed.plugin_id, "a");
        h.dismiss(failed.id);
        assert!(h.automatic_failure().is_none());
    }

    #[test]
    fn capture_toggles_switch_their_plugin_on() {
        let mut h = PluginHost::default();
        let mut m = manifest("cap");
        m.kind = Some("capture".into());
        m.capture_sources = Some(vec![
            "screen".into(),
            "camera".into(),
            "system-audio".into(),
        ]);
        m.settings = ["screen", "camera"]
            .iter()
            .map(|k| PluginSetting {
                key: (*k).into(),
                title: (*k).into(),
                kind: SettingKind::Bool,
                default: JsonValue::Bool(true),
                options: vec![],
                help: None,
            })
            .collect();
        h.plugins.push(Plugin {
            folder: PathBuf::new(),
            manifest: m,
        });
        let toggles = h.capture_toggles();
        assert_eq!(
            toggles
                .iter()
                .map(|t| t.source.as_str())
                .collect::<Vec<_>>(),
            ["screen", "camera"]
        );
        assert!(!h.is_on(&toggles[0]), "plugin off");
        h.set_on(&toggles[1], true);
        assert!(h.is_enabled("cap"));
        assert!(h.is_on(&toggles[1]));
        assert!(!h.is_on(&toggles[0]), "only the clicked source records");
    }
}
