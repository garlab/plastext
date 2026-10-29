use gpui::{Context, EntityId, Global, Task};
use plastext_platform::tex::{Config, Engine, Session, State};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Default)]
struct Permissions(HashMap<PathBuf, (Engine, Vec<PathBuf>)>);
impl Global for Permissions {}

/// Shared by split views. Permission is session-only, scoped to an input folder.
pub(super) struct Model {
    pub input: PathBuf,
    pub engine: Option<Engine>,
    pub input_folders: Vec<PathBuf>,
    pub state: State,
    pub paused: bool,
    pub enabled: bool,
    visible: HashSet<EntityId>,
    setup: bool,
    session: Option<Session>,
    task: Option<Task<()>>,
    discovery: Option<Task<()>>,
}
impl Model {
    pub fn new(input: PathBuf, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Permissions>() {
            cx.set_global(Permissions::default());
        }
        Self {
            input,
            engine: None,
            input_folders: Vec::new(),
            state: State {
                status: "Preview disabled".into(),
                error: None,
                output: None,
                revision: 0,
            },
            paused: false,
            enabled: false,
            visible: HashSet::new(),
            setup: false,
            session: None,
            task: None,
            discovery: None,
        }
    }
    pub fn setup(&mut self, cx: &mut Context<Self>) {
        if self.setup {
            return;
        }
        self.setup = true;
        if let Some((engine, inputs)) = self
            .input
            .parent()
            .and_then(|root| cx.global::<Permissions>().0.get(root))
            .cloned()
        {
            self.engine = Some(engine);
            self.input_folders = inputs;
            self.enable(cx);
            return;
        }
        self.state.status = "Choose an installed compiler to enable this folder".into();
        self.discovery = Some(cx.spawn(async |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { Engine::discover() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.discovery = None;
                match result {
                    Ok(engine) => this.engine = Some(engine),
                    Err(error) => this.state.error = Some(error),
                }
                cx.notify();
            });
        }));
    }
    pub fn select_engine(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.discovery = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Engine::at(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.discovery = None;
                match result {
                    Ok(engine) => {
                        this.engine = Some(engine);
                        this.enable(cx);
                    }
                    Err(error) => this.state.error = Some(error),
                }
                cx.notify();
            });
        }));
    }
    pub fn enable(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            return;
        }
        let Some(engine) = self.engine.clone() else {
            return;
        };
        self.enabled = true;
        self.state.error = None;
        self.state.status = "Starting local preview…".into();
        let input = self.input.clone();
        let inputs = self.input_folders.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let config = Config::with_inputs(&input, engine, &inputs)?;
                    let permission = (config.root.clone(), (config.engine.clone(), inputs));
                    Session::start(config).map(|(session, events)| (permission, session, events))
                })
                .await;
            let events = this
                .update(cx, |this, cx| match result {
                    Ok(((root, engine), session, events)) => {
                        cx.global_mut::<Permissions>().0.insert(root, engine);
                        session.set_paused(this.paused || this.visible.is_empty());
                        this.accept(session.state());
                        this.session = Some(session);
                        cx.notify();
                        Some(events)
                    }
                    Err(error) => {
                        this.enabled = false;
                        this.state.status = "Preview could not start".into();
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
                            this.accept(session.state());
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
            session.set_paused(self.paused || self.visible.is_empty());
        }
    }
    fn accept(&mut self, mut state: State) {
        if state.output.is_none() {
            state.output = self.state.output.clone();
        }
        self.state = state;
    }
    pub fn add_input_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.input_folders.contains(&path) {
            return;
        }
        self.input_folders.push(path);
        if self.enabled {
            self.session = None;
            self.task = None;
            self.enabled = false;
            self.enable(cx);
        }
        cx.notify();
    }
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.paused = !self.paused;
        if let Some(session) = &self.session {
            session.set_paused(self.paused || self.visible.is_empty());
        }
        cx.notify();
    }
    pub fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.paused = false;
        if let Some(session) = &self.session {
            session.set_paused(false);
            session.refresh();
        } else {
            self.enable(cx);
        }
        cx.notify();
    }
    pub fn choose_root(&mut self, input: PathBuf, cx: &mut Context<Self>) {
        self.session = None;
        self.task = None;
        self.discovery = None;
        self.input = input;
        self.engine = None;
        self.input_folders.clear();
        self.enabled = false;
        self.setup = false;
        self.state.status = "Preview disabled".into();
        self.state.error = None;
        self.state.output = None;
        self.state.revision = 0;
        self.setup(cx);
        cx.notify();
    }
}
