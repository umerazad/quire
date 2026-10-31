use crate::{FormatError, Job, Limits, Parser};
use rquickjs::{Context, Function, Promise, Runtime, Value};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Instant,
};

macro_rules! bundle {
    ($path:literal) => {
        include_str!(concat!("../../../vendor/prettier/", $path))
    };
}

/// Prettier's UMD bundles in the order rt passes its plugins, so `Object.values(prettierPlugins)`
/// matches rt's plugin list.
pub(crate) const BUNDLES: [(&str, &str); 8] = [
    ("standalone.js", bundle!("standalone.js")),
    ("plugins/babel.js", bundle!("plugins/babel.js")),
    ("plugins/estree.js", bundle!("plugins/estree.js")),
    ("plugins/typescript.js", bundle!("plugins/typescript.js")),
    ("plugins/postcss.js", bundle!("plugins/postcss.js")),
    ("plugins/html.js", bundle!("plugins/html.js")),
    ("plugins/markdown.js", bundle!("plugins/markdown.js")),
    ("plugins/yaml.js", bundle!("plugins/yaml.js")),
];

/// rt's call in `src/editor/code-tools.ts`: every plugin and `tabWidth: 2`.
const GLUE: &str = r#"
globalThis.__quireFormat = (text, parser) =>
  prettier.format(text, { parser, plugins: Object.values(prettierPlugins), tabWidth: 2 });
globalThis.__quireDescribe = (error) => String(error);
"#;

enum Stop {
    Cancelled,
    TimedOut,
}

/// What the interrupt handler checks: the running request's cancel flag and deadline.
#[derive(Default)]
struct Watch {
    running: Mutex<Option<(Arc<AtomicBool>, Instant)>>,
    stopped: Mutex<Option<Stop>>,
}

impl Watch {
    fn start(&self, cancel: Arc<AtomicBool>, deadline: Instant) {
        *lock(&self.stopped) = None;
        *lock(&self.running) = Some((cancel, deadline));
    }

    fn finish(&self) -> Option<Stop> {
        *lock(&self.running) = None;
        lock(&self.stopped).take()
    }

    fn interrupt(&self) -> bool {
        let stop = match &*lock(&self.running) {
            Some((cancel, _)) if cancel.load(Ordering::Relaxed) => Stop::Cancelled,
            Some((_, deadline)) if Instant::now() >= *deadline => Stop::TimedOut,
            _ => return false,
        };
        *lock(&self.stopped) = Some(stop);
        true
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Engine {
    context: Context,
    _runtime: Runtime,
}

impl Engine {
    fn new(limits: &Limits, watch: Arc<Watch>) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_max_stack_size(8 * 1024 * 1024);
        runtime.set_memory_limit(limits.memory_bytes);
        runtime.set_interrupt_handler(Some(Box::new(move || watch.interrupt())));
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        context.with(|ctx| {
            BUNDLES
                .iter()
                .chain([("glue", GLUE)].iter())
                .try_for_each(|(name, source)| {
                    ctx.eval::<(), _>(*source)
                        .map_err(|error| format!("loading {name} failed with {error}"))
                })
        })?;
        Ok(Self {
            context,
            _runtime: runtime,
        })
    }

    fn format(&self, text: &str, parser: Parser) -> Result<String, String> {
        self.context.with(|ctx| {
            let run = || -> rquickjs::Result<String> {
                let format: Function = ctx.globals().get("__quireFormat")?;
                let promise: Promise = format.call((text, parser.name()))?;
                promise.finish::<String>()
            };
            run().map_err(|error| match error {
                rquickjs::Error::Exception => {
                    let exception: Value = ctx.catch();
                    ctx.globals()
                        .get::<_, Function>("__quireDescribe")
                        .and_then(|describe| describe.call::<_, String>((exception,)))
                        .unwrap_or_else(|_| "an unreadable JavaScript exception".into())
                }
                other => other.to_string(),
            })
        })
    }
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

/// The formatter thread: runs each request, rebuilding the engine after any failure other than
/// Prettier rejecting the input.
pub(crate) fn serve(jobs: mpsc::Receiver<Job>, limits: Limits) {
    let watch = Arc::new(Watch::default());
    let mut engine: Option<Engine> = None;
    for job in jobs {
        if job.cancel.load(Ordering::Relaxed) {
            job.reply.send(Err(FormatError::Cancelled)).ok();
            continue;
        }
        let ready = match engine.take() {
            Some(ready) => Ok(ready),
            None => Engine::new(&limits, watch.clone()),
        };
        let ready = match ready {
            Ok(ready) => ready,
            Err(error) => {
                job.reply.send(Err(FormatError::Engine(error))).ok();
                continue;
            }
        };
        watch.start(job.cancel.clone(), Instant::now() + limits.timeout);
        let result = ready.format(&job.text, job.parser);
        let result = match (watch.finish(), result) {
            (Some(Stop::Cancelled), _) => Err(FormatError::Cancelled),
            (Some(Stop::TimedOut), _) => Err(FormatError::TimedOut(limits.timeout)),
            (None, Ok(text)) => Ok(text),
            (None, Err(message)) if message.contains("out of memory") => Err(FormatError::Engine(
                "it ran out of memory on this block".into(),
            )),
            (None, Err(message)) => Err(FormatError::Invalid(first_line(&message))),
        };
        if matches!(result, Ok(_) | Err(FormatError::Invalid(_))) {
            engine = Some(ready);
        }
        job.reply.send(result).ok();
    }
}
