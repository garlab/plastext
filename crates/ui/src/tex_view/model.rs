use gpui::{Context, EntityId, Task};
use plastext_platform::tex::{Config, Engine, Session, State};
use plastext_settings::Tex;
use std::{collections::HashSet, path::PathBuf, time::Duration};

/// Splits share one build while choosing Source/Preview independently.
pub(super) struct Model {
    pub input: PathBuf,
    source: PathBuf,
    workspace: Option<PathBuf>,
    settings: Tex,
    root_override: Option<PathBuf>,
    input_folders: Vec<PathBuf>,
    pub state: State,
    visible: HashSet<EntityId>,
    setup: bool,
    session: Option<Session>,
    task: Option<Task<()>>,
    slow_notice: Option<Task<()>>,
}
impl Model {
    pub fn new(input: PathBuf, workspace: Option<PathBuf>, settings: Tex) -> Self {
        Self {
            source: input.clone(),
            input,
            workspace,
            settings,
            root_override: None,
            input_folders: Vec::new(),
            state: State {
                status: "Preparing preview…".into(),
                error: None,
                output: None,
                revision: 0,
                started: None,
            },
            visible: HashSet::new(),
            setup: false,
            session: None,
            task: None,
            slow_notice: None,
        }
    }
    pub fn setup(&mut self, cx: &mut Context<Self>) {
        if self.setup {
            return;
        }
        self.setup = true;
        self.state.error = None;
        self.state.status = "Preparing preview…".into();
        let input = self
            .root_override
            .clone()
            .unwrap_or_else(|| self.source.clone());
        let workspace = self.workspace.clone();
        let compiler = self.settings.compiler.clone();
        let inputs = self.input_folders.clone();
        let explicit_root = self.root_override.is_some();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let engine = match compiler {
                        Some(path) => Engine::at(&path)?,
                        None => Engine::discover()?,
                    };
                    let config = if explicit_root {
                        Config::automatic_root(&input, workspace.as_deref(), engine)?
                    } else {
                        Config::automatic(&input, workspace.as_deref(), engine)?
                    }
                    .with_additional_inputs(&inputs)?;
                    let root = config.input.clone();
                    Session::start(config).map(|(session, events)| (root, session, events))
                })
                .await;
            let events = this
                .update(cx, |this, cx| match result {
                    Ok((root, session, events)) => {
                        this.input = root;
                        session.set_paused(this.visible.is_empty());
                        this.accept(session.state(), cx);
                        this.session = Some(session);
                        cx.notify();
                        Some(events)
                    }
                    Err(error) => {
                        this.state.status = "Preview unavailable".into();
                        this.state.error = Some(error);
                        cx.notify();
                        None
                    }
                })
                .ok()
                .flatten();
            let Some(events) = events else {
                return;
            };
            while events.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| {
                        if let Some(session) = &this.session {
                            this.accept(session.state(), cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }
    pub fn visible(&mut self, owner: EntityId, visible: bool) {
        if visible {
            self.visible.insert(owner);
        } else {
            self.visible.remove(&owner);
        }
        if let Some(session) = &self.session {
            session.set_paused(self.visible.is_empty());
        }
    }
    fn accept(&mut self, mut state: State, cx: &mut Context<Self>) {
        if state.output.is_none() {
            state.output = self.state.output.clone();
        }
        if state.started != self.state.started {
            self.slow_notice = state.started.map(|started| {
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_secs(10).saturating_sub(started.elapsed()))
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
            });
        }
        self.state = state;
    }
    pub fn slow(&self) -> bool {
        self.state
            .started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(10))
    }
    pub fn cancel(&mut self) {
        if let Some(session) = &self.session {
            session.cancel_current();
        }
    }
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        // Retry also re-reads advanced project settings and redetects the root.
        self.session = None;
        self.task = None;
        self.slow_notice = None;
        self.state.started = None;
        self.setup = false;
        if !self.visible.is_empty() {
            self.setup(cx);
        }
        cx.notify();
    }
    pub fn select_engine(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.settings.compiler = Some(path);
        self.rebuild(cx);
    }
    pub fn add_input_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.input_folders.contains(&path) {
            self.input_folders.push(path);
        }
        self.rebuild(cx);
    }
    pub fn choose_root(&mut self, input: PathBuf, cx: &mut Context<Self>) {
        self.root_override = Some(input.clone());
        self.input = input;
        self.state.output = None;
        self.rebuild(cx);
    }
}
