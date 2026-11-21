mod csv;
mod input;
mod name;
mod output;
mod scan;
mod sessions;

use std::{
    env,
    error::Error,
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::Instant,
};

fn phase(name: &str) {
    if env::var_os("SHOP_PROFILE").is_some() {
        static LAST: OnceLock<Mutex<Instant>> = OnceLock::new();
        let mut last = LAST
            .get_or_init(|| Mutex::new(Instant::now()))
            .lock()
            .unwrap();
        eprintln!("{name}: {:.3} s", last.elapsed().as_secs_f64());
        *last = Instant::now();
    }
}
fn run() -> Result<(), Box<dyn Error>> {
    let path = |key: &str, pos, default: &str| {
        env::var_os(key)
            .or_else(|| env::args_os().nth(pos))
            .map(PathBuf::from)
            .unwrap_or_else(|| default.into())
    };
    let data = path("DATA_DIR", 1, "/data");
    let out = path("OUTPUT_DIR", 2, "/output");
    let config = |key: &str, default: usize| -> Result<usize, Box<dyn Error>> {
        Ok(match env::var(key) {
            Ok(n) => n.parse::<usize>()?.max(1),
            Err(_) => default,
        })
    };
    phase("start");
    let workers = config("NCPU", 8)?;
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .build_global()?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(data.join("manifest.json"))?)?;
    let mut clock = input::Clock::new();
    let mut time = |key: &str| {
        manifest[key]
            .as_str()
            .and_then(|s| clock.parse(s.as_bytes()))
            .ok_or("invalid manifest timestamp")
    };
    let window = (time("window_start")?, time("window_end")?);
    if window.0 >= window.1 {
        return Err("empty manifest window".into());
    }
    let files = input::Files::open(&data)?;
    let catalog = input::Catalog::new(&files);
    phase("open inputs");
    let people = scan::prepare(&files, &catalog, window);
    phase("group people");
    let results = sessions::summarize(people, &catalog);
    phase("sessions");
    let scratch = env::var_os("SCRATCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    output::write(
        &out,
        results,
        &catalog,
        workers,
        config("MEM_GB", 24)?,
        &scratch,
    )?;
    phase("DuckDB aggregate and Parquet");
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("shopping-sessions: {e}");
        std::process::exit(1);
    }
}
