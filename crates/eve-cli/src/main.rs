//! Command line interface for the perception and semantic layers.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand};
use eve_memory::{
    discover_clients, save_process_sample, TreeLimits, UiReader,
};

#[derive(Parser)]
#[command(
    name = "eve-cli",
    version,
    about = "EVE Online perception tooling: client discovery, memory dumps, UI tree reads"
)]
struct Cli {
    /// Log filter (RUST_LOG syntax), e.g. "info" or "eve_memory=debug".
    #[arg(long, default_value = "info", global = true)]
    log: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List running EVE Online clients (all flavors) with windows.
    Clients,
    /// Record a full memory dump of a client for offline replay.
    Dump {
        /// Process id of the client.
        #[arg(long)]
        pid: u32,
        /// Output directory (created if missing).
        #[arg(long, default_value = "samples")]
        out_dir: PathBuf,
        /// Wait this many seconds before copying memory.
        #[arg(long)]
        delay: Option<f64>,
    },
    /// Read the UI tree from a live client or a recorded sample.
    Read {
        /// Live process id (mutually exclusive with --sample).
        #[arg(long, conflicts_with = "sample")]
        pid: Option<u32>,
        /// Replay from a recorded sample archive.
        #[arg(long)]
        sample: Option<PathBuf>,
        /// Skip discovery and read the tree at this address (0x-hex or decimal).
        #[arg(long)]
        root_address: Option<String>,
        /// Output JSON file ("-" for stdout).
        #[arg(long)]
        output_file: Option<String>,
        /// Omit non-whitelisted dict keys from the output.
        #[arg(long)]
        remove_other_dict_entries: bool,
        /// Run N warmup reads before the measured one.
        #[arg(long)]
        warmup_iterations: Option<u32>,
    },
    /// Parse a UI tree JSON (or read live/sample) and print the semantic snapshot.
    Snapshot {
        #[arg(long, conflicts_with = "sample")]
        pid: Option<u32>,
        #[arg(long)]
        sample: Option<PathBuf>,
    },
    /// List Python types found in memory with instance counts.
    Types {
        #[arg(long, conflicts_with = "sample")]
        pid: Option<u32>,
        #[arg(long)]
        sample: Option<PathBuf>,
        /// Print at most this many types.
        #[arg(long, default_value = "80")]
        limit: usize,
    },
    /// Report where a click at window-client (x, y) would land.
    HitTest {
        #[arg(long, conflicts_with = "sample")]
        pid: Option<u32>,
        #[arg(long)]
        sample: Option<PathBuf>,
        #[arg(long)]
        x: i64,
        #[arg(long)]
        y: i64,
    },
    /// Capture a client's window to a PNG (optionally activating it first).
    Shot {
        #[arg(long)]
        pid: u32,
        #[arg(long, default_value = "shot.png")]
        out: PathBuf,
        /// Bring the window to the foreground before capturing.
        #[arg(long)]
        activate: bool,
    },
    /// Benchmark consecutive full-tree reads (post-warmup) with
    /// min/p50/p95/max latencies and effective fps.
    Bench {
        #[arg(long, conflicts_with = "sample")]
        pid: Option<u32>,
        #[arg(long)]
        sample: Option<PathBuf>,
        /// Frames to measure after warmup.
        #[arg(long, default_value_t = 100)]
        frames: u32,
    },
    /// Build the full icon→module-name table from the EVE static data
    /// data (local client or SDE fallback) into the resource overlay.
    Icons {
        #[command(subcommand)]
        command: IconsCommand,
    },
    /// Record a synchronized session: human input + semantic snapshots
    /// + per-client window video (60 fps HEVC). Stop with Ctrl-C.
    Record {
        /// PIDs to record (repeatable); default: every running client.
        #[arg(long)]
        pid: Vec<u32>,
        /// Output directory; default record-<YYYYmmdd-HHMMSS>.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Observation polling interval in milliseconds.
        #[arg(long, default_value_t = 500)]
        interval_ms: u64,
        /// Minimum spacing of mouse-move samples (16 ≈ 60 fps, 0 = all).
        #[arg(long, default_value_t = 16)]
        move_every_ms: u64,
        /// Video frame rate.
        #[arg(long, default_value_t = 15)]
        video_fps: u32,
        /// Encoder backend: auto (default) or a specific name
        /// (h264_nvenc / h264_amf / h264_qsv / libx264).
        #[arg(long)]
        encoder: Option<String>,
        /// Target resolution budget: 720p (default), 1080p, or native.
        /// Scales down preserving aspect; pixel count stays <= budget.
        #[arg(long, default_value = "720p")]
        video_size: String,
        /// Disable the video stream (no ffmpeg needed).
        #[arg(long)]
        no_video: bool,
        /// Stop automatically after this many seconds.
        #[arg(long)]
        duration_sec: Option<u64>,
    },
}

#[derive(Subcommand)]
enum IconsCommand {
    /// Refresh the type/icon/name resource tables. Default source is
    /// the LOCAL client (its own FSD data + zh localization via the
    /// client's official loaders — includes 国服 exclusives); the
    /// fuzzwork SDE download remains as a fallback for machines
    /// without a client install.
    Update {
        /// Data source: `client` (default, local game only) or `sde`.
        #[arg(long, default_value = "client")]
        source: String,
        /// Client flavor dir under the shared cache (client source).
        #[arg(long, default_value = "infinity")]
        flavor: String,
        /// Localization language (client source): zh, en.
        #[arg(long, default_value = "zh")]
        lang: String,
        /// Shared cache root (client source).
        #[arg(long, default_value = r"C:\EVE\SharedCache")]
        client_root: PathBuf,
        /// Write the repo baseline (data/types.<flavor>.json[.gz])
        /// instead of the user overlay — maintainer flow.
        #[arg(long)]
        write_baseline: bool,
        /// SDE download cache dir (sde source).
        #[arg(long, default_value = "samples/sde")]
        cache_dir: PathBuf,
    },
    /// Resolve one icon fingerprint through the current table.
    Show {
        icon: String,
        /// Also report which resource layer answered.
        #[arg(long)]
        verbose: bool,
    },
    /// Export the icon PNGs themselves from the game's shared cache.
    Export {
        /// Output directory for the PNGs (named by icon id).
        #[arg(long, default_value = "icons")]
        out: PathBuf,
        /// Shared cache root (e.g. C:\EVE\SharedCache); auto-detected
        /// from a running client when omitted.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Export at most N icons (0 = all in the table).
        #[arg(long, default_value = "0")]
        limit: usize,
    },
}

fn main() {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| cli.log.clone().into()),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    match cli.command {
        Command::Clients => list_clients(),
        Command::Dump {
            pid,
            out_dir,
            delay,
        } => dump_client(pid, out_dir, delay),
        Command::Read {
            pid,
            sample,
            root_address,
            output_file,
            remove_other_dict_entries,
            warmup_iterations,
        } => read_tree(ReadArgs {
            pid,
            sample,
            root_address,
            output_file,
            remove_other_dict_entries,
            warmup_iterations,
        }),
        Command::Snapshot { pid, sample } => snapshot(pid, sample),
        Command::Types { pid, sample, limit } => types(pid, sample, limit),
        Command::HitTest { pid, sample, x, y } => hit_test_cmd(pid, sample, x, y),
        Command::Shot { pid, out, activate } => shot(pid, out, activate),
        Command::Bench { pid, sample, frames } => bench(pid, sample, frames),
        Command::Icons { command } => icons(command),
        Command::Record {
            pid,
            out,
            interval_ms,
            move_every_ms,
            video_fps,
            encoder,
            video_size,
            no_video,
            duration_sec,
        } => record(eve_recorder::RecorderConfig {
            pids: pid,
            out_dir: out.unwrap_or_else(default_record_dir),
            interval_ms,
            move_every_ms,
            video_fps,
            encoder,
            video_size: eve_recorder::VideoSize::parse(&video_size).unwrap_or_else(|| {
                eprintln!("invalid --video-size {video_size:?} (use 720p|1080p|native)");
                std::process::exit(1);
            }),
            no_video,
            duration_sec,
        }),
    }
}

/// Default session directory: `record-<YYYYmmdd-HHMMSS>` in the cwd.
fn default_record_dir() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil time from the epoch seconds via the days-since-epoch method;
    // only used for a directory name, ±a minute is irrelevant.
    let days = now / 86_400;
    let secs_of_day = now % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    PathBuf::from(format!(
        "record-{:04}{:02}{:02}-{:02}{:02}{:02}",
        year,
        month,
        day,
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    ))
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn record(config: eve_recorder::RecorderConfig) {
    println!(
        "recording to {} — press Ctrl-C to stop (graceful flush)",
        config.out_dir.display()
    );
    match eve_recorder::record_session(config) {
        Ok(summary) => {
            println!("session written to {}", summary.out_dir.display());
            println!(
                "  input: {} events ({} dropped)",
                summary.input.events_written, summary.input.events_dropped
            );
            for c in &summary.clients {
                let video = c
                    .video
                    .as_ref()
                    .map(|v| {
                        format!(
                            "video {} frames @ {} fps [{}] {}x{} ({}x{} native, {:.3} scale) ({} dropped){}",
                            v.frames_written,
                            v.fps,
                            v.encoder,
                            v.width,
                            v.height,
                            v.native_width,
                            v.native_height,
                            v.scale,
                            v.frames_dropped,
                            v.encoder_error.as_deref().map(|e| format!(" ERR: {e}")).unwrap_or_default()
                        )
                    })
                    .unwrap_or_else(|| "no video".into());
                println!(
                    "  pid {} ({}): {} frames, {} failed, {} slow reads — {}",
                    c.pid, c.flavor, c.observe.frames, c.observe.failed_reads, c.observe.slow_reads, video
                );
                if let Some(v) = c.video.as_ref() {
                    if v.frames_dropped > 0 {
                        println!("  ⚠ pid {}: encoder dropped {} video frames (see warnings)", c.pid, v.frames_dropped);
                    }
                }
                if c.observe.slow_reads > 0 {
                    println!("  ⚠ pid {}: {} observation reads slower than interval (frame rate lagged)", c.pid, c.observe.slow_reads);
                }
            }
        }
        Err(e) => {
            eprintln!("record failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Fuzzwork per-table CSV mirrors of the EVE SDE (latest TQ dump, plain
/// CSV). Note: TQ data — China-server exclusives and localized names are
/// NOT covered here; supplement via `--source client` (future) or the
/// manual overrides.
const SDE_BASE: &str = "https://www.fuzzwork.co.uk/dump/latest/csv";

fn icons(command: IconsCommand) {
    match command {
        IconsCommand::Update {
            source,
            flavor,
            lang,
            client_root,
            write_baseline,
            cache_dir,
        } => icons_update(&source, &flavor, &lang, &client_root, write_baseline, &cache_dir),
        IconsCommand::Show { icon, verbose } => {
            let store = eve_semantics::resources::ResourceStore::global();
            match eve_semantics::icons::module_name_from_icon(&icon) {
                Some(name) => {
                    if verbose {
                        println!("{icon} → {name}   [{}]", store.loaded_from);
                    } else {
                        println!("{icon} → {name}");
                    }
                }
                None => println!("{icon} → (unmapped)"),
            }
        }
        IconsCommand::Export { out, root, limit } => icons_export(out, root, limit),
    }
}

fn icons_export(out: PathBuf, root: Option<PathBuf>, limit: usize) {
    match icons_export_inner(&out, root.as_deref(), limit) {
        Ok(count) => println!("exported {count} icons → {}", out.display()),
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    }
}

fn icons_export_inner(out: &Path, root: Option<&Path>, limit: usize) -> Result<usize, String> {
    // Locate the shared cache: explicit root, or derive from a running
    // client (…\SharedCache\<flavor>\bin64\exefile.exe → SharedCache).
    let (cache_root, flavor) = match root {
        Some(root) => (root.to_path_buf(), "infinity".to_string()),
        None => {
            let clients = discover_clients().map_err(|e| e.to_string())?;
            let client = clients
                .first()
                .ok_or("no running client; pass --root <SharedCache dir>")?;
            let cache_root = eve_memory::ResourceCache::root_from_client_exe(&client.exe_path)
                .ok_or_else(|| {
                    format!(
                        "cannot derive shared cache root from {}",
                        client.exe_path.display()
                    )
                })?;
            let flavor = client
                .exe_path
                .parent()
                .and_then(|bin| bin.parent())
                .and_then(|dir| dir.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "infinity".to_string());
            (cache_root, flavor)
        }
    };
    println!("shared cache: {} ({flavor})", cache_root.display());
    let cache =
        eve_memory::ResourceCache::open(&cache_root, &flavor).map_err(|e| e.to_string())?;
    println!("resource index: {} entries", cache.len());

    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let entries = eve_semantics::icons::all_icon_entries();
    let selected: Vec<&(&str, &str)> = if limit > 0 {
        entries.iter().take(limit).collect()
    } else {
        entries.iter().collect()
    };
    let mut exported = 0usize;
    let mut missing = 0usize;
    for (icon, _name) in selected {
        // icons: `res:/ui/texture/icons/<id>.png` → `<id>.png`
        let Some(id) = icon.rsplit('/').next() else { continue };
        let target = out.join(id);
        if target.exists() {
            exported += 1;
            continue;
        }
        match cache.read(icon) {
            Ok(bytes) => {
                std::fs::write(&target, bytes).map_err(|e| e.to_string())?;
                exported += 1;
            }
            Err(_) => missing += 1,
        }
    }
    if missing > 0 {
        println!("skipped {missing} icons missing from the cache");
    }
    Ok(exported)
}

/// py2 payload shared with scripts/derive_client_resources.py — the
/// client's official FSD decoders are Python 2 extension modules, so
/// the derive step must run under a stock CPython 2.7.
const PY2_DERIVE_PAYLOAD: &str = include_str!("../../../scripts/py2/derive_payload.py");
const PY27_MSI_URL: &str = "https://www.python.org/ftp/python/2.7.18/python-2.7.18.amd64.msi";

fn icons_update(
    source: &str,
    flavor: &str,
    lang: &str,
    client_root: &Path,
    write_baseline: bool,
    cache_dir: &Path,
) {
    let result = match source {
        "client" => update_from_client(flavor, lang, client_root),
        "sde" => update_from_sde(cache_dir),
        other => Err(format!("unknown source {other:?} (client | sde)")),
    };
    let mut doc = match result {
        Ok(doc) => doc,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    let count = doc["types"].as_object().map(|t| t.len()).unwrap_or(0);
    let text = serde_json::to_string(&doc).unwrap();
    if write_baseline {
        let json_path = std::path::Path::new("data").join(format!("types.{flavor}.json"));
        let gz_path = json_path.with_extension("json.gz");
        std::fs::write(&json_path, &text).map_err(|e| e.to_string()).err();
        let gz = std::fs::File::create(&gz_path).map_err(|e| e.to_string()).unwrap();
        let mut enc = flate2::write::GzEncoder::new(gz, flate2::Compression::best());
        enc.write_all(text.as_bytes()).map_err(|e| e.to_string()).unwrap();
        enc.finish().map_err(|e| e.to_string()).unwrap();
        println!(
            "baseline: {count} types → {} (+ .gz twin); rebuild to embed",
            json_path.display()
        );
    } else {
        let Some(path) = eve_semantics::resources::overlay_path(flavor) else {
            eprintln!("error: no writable data dir (LOCALAPPDATA / EVE_NG_DATA_DIR)");
            std::process::exit(1);
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string()).err();
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &text).map_err(|e| e.to_string()).err();
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string()).err();
        println!(
            "overlay: {count} types → {} (takes effect on next process start)",
            path.display()
        );
    }
    if let Some(meta) = doc["meta"].as_object_mut() {
        meta.insert("flavor".into(), flavor.into());
        meta.insert(
            "written_at_unix_ms".into(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or_default()
                .into(),
        );
    }
    let _ = &mut doc;
}

/// SDE fallback: fuzzwork TQ tables → the same JSON schema (English
/// names, no 国服 exclusives — the client source is strictly better
/// whenever a game install exists).
fn update_from_sde(cache_dir: &Path) -> Result<serde_json::Value, String> {
    icons_update_inner(cache_dir)
}

/// Derive from the LOCAL client: stage the FSD binaries + localization
/// pickle from the shared cache, bootstrap a py2.7 tool, and run the
/// client's own loader pyds.
fn update_from_client(
    flavor: &str,
    lang: &str,
    client_root: &Path,
) -> Result<serde_json::Value, String> {
    let lang_tag = match lang {
        "zh" => "zh",
        "en" => "en-us",
        other => return Err(format!("unsupported lang {other:?} (zh | en)")),
    };
    let flavor_dir = client_root.join(flavor);
    let index = std::fs::read_to_string(flavor_dir.join("resfileindex.txt"))
        .map_err(|e| format!("resfileindex: {e}"))?;
    let want: &[(&str, &str)] = &[
        ("types", "res:/staticdata/types.fsdbinary"),
        ("iconids", "res:/staticdata/iconids.fsdbinary"),
        ("groups", "res:/staticdata/groups.fsdbinary"),
        ("categories", "res:/staticdata/categories.fsdbinary"),
    ];
    let loc_res = format!("res:/localizationfsd/localization_fsd_{lang_tag}.pickle");
    let resfiles = client_root.join("ResFiles");
    let mut staged: Vec<(&str, PathBuf)> = Vec::new();
    let mut loc = None;
    for line in index.lines() {
        let mut parts = line.split(',');
        let (Some(res), Some(blob)) = (parts.next(), parts.next()) else {
            continue;
        };
        if let Some((name, _)) = want.iter().find(|(_, r)| *r == res) {
            staged.push((name, resfiles.join(blob)));
        } else if res == loc_res {
            loc = Some(resfiles.join(blob));
        }
    }
    let missing: Vec<&str> = want
        .iter()
        .filter(|(name, _)| !staged.iter().any(|(n, _)| n == name))
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        return Err(format!("staticdata tables missing from index: {missing:?}"));
    }
    let loc = loc.ok_or("localization pickle missing from index")?;
    for (_, p) in &staged {
        if !p.is_file() {
            return Err(format!("indexed file missing on disk: {}", p.display()));
        }
    }
    let bin64 = flavor_dir.join("bin64");
    if !bin64.join("typesLoader.pyd").is_file() {
        return Err(format!("client loaders not found under {}", bin64.display()));
    }

    let py27 = ensure_py27(&bin64)?;
    let tmp = std::env::temp_dir().join(format!("eve-ng-icons-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let mut args: Vec<String> = vec!["--bin64".into(), bin64.display().to_string()];
    for (name, src) in &staged {
        let dst = tmp.join(format!("{name}.fsdbinary"));
        std::fs::copy(src, &dst).map_err(|e| e.to_string())?;
        args.push(format!("--{name}"));
        args.push(dst.display().to_string());
    }
    let loc_dst = tmp.join("loc.pickle");
    std::fs::copy(&loc, &loc_dst).map_err(|e| e.to_string())?;
    args.push("--localization".into());
    args.push(loc_dst.display().to_string());
    let derived = tmp.join("derived.json");
    args.push("--out".into());
    args.push(derived.display().to_string());

    let payload_path = tmp.join("payload.py");
    std::fs::write(&payload_path, PY2_DERIVE_PAYLOAD).map_err(|e| e.to_string())?;
    println!("running client FSD loaders (py2.7)…");
    let output = std::process::Command::new(&py27)
        .arg(&payload_path)
        .args(&args)
        .env(
            "PATH",
            format!(
                "{};{};C:\\Windows\\System32;C:\\Windows",
                bin64.display(),
                py27.parent().unwrap().display()
            ),
        )
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .map_err(|e| format!("run py2.7: {e}"))?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() {
        return Err(format!(
            "py2 derive failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let bytes = std::fs::read(&derived).map_err(|e| e.to_string())?;
    std::fs::remove_dir_all(&tmp).ok();
    let mut doc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("derived json: {e}"))?;
    if let Some(meta) = doc["meta"].as_object_mut() {
        meta.insert("lang".into(), lang.into());
        meta.insert(
            "derived_from".into(),
            "local client (official FSD loaders)".into(),
        );
        meta.insert("client_root".into(), client_root.display().to_string().into());
    }
    Ok(doc)
}

/// Bootstrap the py2.7 extractor runtime. Lookup order:
///
/// 1. the copy bundled inside the Python package (`eve_proxy_ng/tools/py27`,
///    next to the exe's dir in the wheel layout) — offline-first;
/// 2. a previously bootstrapped copy under the user data dir;
/// 3. download + administrative extract (one-time, ~20 MB).
///
/// **Migration boundary**: the client's `<name>Loader.pyd` decoders are
/// compiled against the client's own Python (2.7 today; a py3 client
/// will ship py3 pyds). The py2-bound pieces are exactly: this runtime,
/// the payload script (written in the 2/3-compatible subset), and the
/// client pyds themselves. `client_python_version` detects the switch;
/// when it fires, only this bootstrap needs a py3 embeddable runtime —
/// payload and data schema stay unchanged.
fn ensure_py27(bin64: &Path) -> Result<PathBuf, String> {
    let (major, minor) = client_python_version(bin64);
    if major != 2 {
        return Err(format!(
            "client uses Python {major}.{minor} loaders; the bundled extractor \
             runtime covers py2 clients only — the derive payload is already \
             2/3-compatible, this bootstrap needs a py{major} runtime \
             (see scripts/py2/derive_payload.py header)"
        ));
    }
    // 1. wheel-bundled copy: <pkg>/bin/eve-cli.exe → <pkg>/tools/py27.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(pkg_dir) = exe.parent().and_then(|d| d.parent()) {
            let candidate = pkg_dir.join("tools").join("py27").join("python.exe");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    let tools = eve_semantics::resources::data_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("eve_proxy_ng"))
        .join("tools");
    let dir = tools.join("py27");
    let python = dir.join("python.exe");
    if python.is_file() {
        return Ok(python);
    }
    let msi = tools.join("python-2.7.18.amd64.msi");
    if !msi.is_file() {
        std::fs::create_dir_all(&tools).map_err(|e| e.to_string())?;
        println!("downloading {PY27_MSI_URL} …");
        let response = ureq::get(PY27_MSI_URL).call().map_err(|e| format!("download py2.7 msi: {e}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(|e| format!("read py2.7 msi: {e}"))?;
        std::fs::write(&msi, &bytes).map_err(|e| e.to_string())?;
    }
    println!("admin-extracting py2.7 → {}", dir.display());
    let status = std::process::Command::new("msiexec")
        .arg("/a")
        .arg(&msi)
        .arg("/qn")
        .arg(format!("TARGETDIR={}", dir.display()))
        .status()
        .map_err(|e| format!("msiexec: {e}"))?;
    if !status.success() || !python.is_file() {
        return Err("py2.7 administrative extract failed".into());
    }
    Ok(python)
}

/// Python generation of a client install, from the pythonXY.dll it
/// ships in bin64 (python27.dll → (2, 7); python312.dll → (3, 12)).
fn client_python_version(bin64: &Path) -> (u8, u8) {
    let entries = std::fs::read_dir(bin64).ok();
    let mut best: Option<(u8, u8)> = None;
    for entry in entries.into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name
            .strip_prefix("python")
            .and_then(|r| r.strip_suffix(".dll"))
        else {
            continue;
        };
        if let Some((major, minor)) = rest.split_once('.') {
            if let (Ok(major), Ok(minor)) = (major.parse::<u8>(), minor.parse::<u8>()) {
                if best.map(|(b, _)| major >= b).unwrap_or(true) {
                    best = Some((major, minor));
                }
            }
        }
    }
    best.unwrap_or((2, 7))
}

fn download_table(cache_dir: &Path, table: &str) -> Result<PathBuf, String> {
    let path = cache_dir.join(format!("{table}.csv"));
    if path.exists() {
        println!("cached: {}", path.display());
        return Ok(path);
    }
    std::fs::create_dir_all(cache_dir).map_err(|e| e.to_string())?;
    let url = format!("{SDE_BASE}/{table}.csv");
    println!("downloading {url} …");
    let response =
        ureq::get(&url).call().map_err(|e| format!("download {table}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {table}: {e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    println!("saved {} bytes", bytes.len());
    Ok(path)
}

fn read_table(path: &Path) -> Result<(csv::StringRecord, Vec<csv::StringRecord>), String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    // Accept both plain and bzip2-compressed tables (older mirrors).
    let text = if raw.starts_with(b"BZ") {
        let mut text = String::new();
        bzip2::read::BzDecoder::new(&raw[..])
            .read_to_string(&mut text)
            .map_err(|e| format!("decompress {}: {e}", path.display()))?;
        text
    } else {
        String::from_utf8_lossy(&raw).into_owned()
    };
    let mut reader = csv::ReaderBuilder::new().from_reader(text.as_bytes());
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let mut rows = Vec::new();
    for record in reader.records() {
        rows.push(record.map_err(|e| e.to_string())?);
    }
    Ok((headers, rows))
}

fn column<'a>(row: &'a csv::StringRecord, headers: &'a csv::StringRecord, name: &str) -> Option<&'a str> {
    let index = headers.iter().position(|header| header == name)?;
    row.get(index)
}

fn icons_update_inner(cache_dir: &Path) -> Result<serde_json::Value, String> {
    // eveIcons: iconID → iconFile
    let (icons_headers, icons_rows) = read_table(&download_table(cache_dir, "eveIcons")?)?;
    for col in ["iconID", "iconFile"] {
        if !icons_headers.iter().any(|h| h == col) {
            return Err(format!(
                "eveIcons table lacks the {col:?} column — delete the stale {} cache and retry",
                cache_dir.display()
            ));
        }
    }
    let mut icon_file_by_id: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for row in &icons_rows {
        let (Some(id), Some(file)) = (
            column(row, &icons_headers, "iconID"),
            column(row, &icons_headers, "iconFile"),
        ) else {
            continue;
        };
        if !file.is_empty() {
            icon_file_by_id.insert(id.to_string(), file.to_string());
        }
    }
    println!("eveIcons: {} icon files", icon_file_by_id.len());

    // invTypes: typeID, typeName, iconID → per-type table.
    let (types_headers, types_rows) = read_table(&download_table(cache_dir, "invTypes")?)?;
    let mut types = serde_json::Map::new();
    let mut skipped_unresolved = 0usize;
    for row in &types_rows {
        let (Some(type_id), Some(name), Some(icon_id)) = (
            column(row, &types_headers, "typeID"),
            column(row, &types_headers, "typeName"),
            column(row, &types_headers, "iconID"),
        ) else {
            continue;
        };
        let Ok(_) = type_id.parse::<u64>() else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let icon = icon_file_by_id
            .get(icon_id)
            .map(|file| normalize_icon_file(file));
        if icon.is_none() {
            skipped_unresolved += 1;
        }
        types.insert(
            type_id.to_string(),
            serde_json::json!({
                "name": name,
                "icon": icon,
            }),
        );
    }
    println!(
        "invTypes: {} types ({} without a resolvable icon — TQ data; no zh names)",
        types.len(),
        skipped_unresolved
    );
    Ok(serde_json::json!({
        "meta": {
            "derived_from": "fuzzwork TQ SDE (fallback)",
            "type_records": types.len(),
        },
        "types": types,
    }))
}

/// SDE `iconFile` values appear as `res:/ui/texture/...` or bare
/// `12_64_10` IDs; normalize to the lowercase full resource path the
/// client exposes via `_texturePath`.
fn normalize_icon_file(file: &str) -> String {
    let file = file.trim().replace('\\', "/").to_ascii_lowercase();
    if file.starts_with("res:") {
        file
    } else {
        format!("res:/ui/texture/icons/{file}.png")
    }
}

fn shot(pid: u32, out: PathBuf, activate: bool) {
    let clients = discover_clients().unwrap_or_default();
    let Some(client) = clients.iter().find(|c| c.pid == pid) else {
        eprintln!("error: no window found for pid {pid}");
        std::process::exit(1);
    };
    let Some(window) = client.window else {
        eprintln!("error: pid {pid} has no window");
        std::process::exit(1);
    };
    if activate {
        window.activate();
        std::thread::sleep(std::time::Duration::from_millis(600));
    }
    match window.capture_client_area() {
        Ok(image) => {
            image.save(&out).expect("save png");
            println!("saved {}", out.display());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn bench(pid: Option<u32>, sample: Option<PathBuf>, frames: u32) {
    let mut reader = match open_reader(pid, sample.as_ref()) {
        Ok(reader) => reader,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    let warm_start = Instant::now();
    let root = match reader.find_ui_root() {
        Ok(root) => root,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "warmup (root discovery + first walk caches): {:?}",
        warm_start.elapsed()
    );
    // Walk manually (same path as read_tree_at) so the block-cache
    // statistics are observable.
    let mut times: Vec<std::time::Duration> = Vec::with_capacity(frames as usize);
    let mut nodes = 0usize;
    let (mut served, mut fetched, mut hits, mut prefetched) = (0u64, 0u64, 0u64, 0u64);
    for _ in 0..frames.max(1) {
        let started = Instant::now();
        let (tree, stats) = match reader.read_tree_at_with_stats(root) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("error on frame {}: {e}", times.len());
                std::process::exit(1);
            }
        };
        nodes = eve_memory::uitree::count_nodes(&tree);
        (served, fetched, hits, prefetched) = stats;
        if times.len() <= 3 || times.len() % 50 == 0 {
            eprintln!(
                "frame {}: | served={} fetched={} hits={} prefetched={} known_pages={}",
                times.len(),
                served,
                fetched,
                hits,
                prefetched,
                reader.known_page_count()
            );
        }
        times.push(started.elapsed());
    }
    times.sort();
    let pick = |q: f64| times[((times.len() as f64 - 1.0) * q).round() as usize];
    let p50 = pick(0.50);
    let p95 = pick(0.95);
    println!(
        "frames={} nodes={} min={:?} p50={:?} p95={:?} max={:?} | median {:.1} fps, p95-limited {:.1} fps",
        times.len(),
        nodes,
        times[0],
        p50,
        p95,
        times[times.len() - 1],
        1000.0 / p50.as_millis() as f64,
        1000.0 / p95.as_millis() as f64,
    );
    println!(
        "frame cache: {} reads served, {} page fetches, {} last-page hits, {} prefetched ({:.1} reads/page)",
        served,
        fetched,
        hits,
        prefetched,
        served as f64 / fetched.max(1) as f64
    );
}

fn hit_test_cmd(pid: Option<u32>, sample: Option<PathBuf>, x: i64, y: i64) {
    let mut reader = match open_reader(pid, sample.as_ref()) {
        Ok(reader) => reader,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    let tree = match reader.read_tree() {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let profile = eve_semantics::FlavorProfile::for_flavor(eve_memory::Flavor::Infinity);
    let regioned = eve_semantics::region::RegionedTree::build(&tree);
    match eve_semantics::interaction::hit_test(&regioned, x, y, profile.layer_order_topmost_first)
    {
        Some(hit) => {
            let report: eve_semantics::interaction::HitTestReport = hit.into();
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("hit report serialization")
            );
        }
        None => println!("null  // no input-taking node at ({x}, {y})"),
    }
}

fn types(pid: Option<u32>, sample: Option<PathBuf>, limit: usize) {
    let reader = match open_reader(pid, sample.as_ref()) {
        Ok(reader) => reader,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    match reader.type_census() {
        Ok(census) => {
            for (name, count) in census.iter().take(limit) {
                println!("{count:>8}  {name}");
            }
            println!("-- {} types total", census.len());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn list_clients() {
    match discover_clients() {
        Ok(clients) if clients.is_empty() => {
            println!("no running EVE Online clients");
        }
        Ok(clients) => {
            for client in &clients {
                println!(
                    "pid {:>7}  {:<22} char {:<20} window 0x{:X}  {}",
                    client.pid,
                    client.flavor.display_name(),
                    client.character_name.as_deref().unwrap_or("-"),
                    client.window_handle_raw().unwrap_or(0),
                    client.exe_path.display()
                );
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

fn dump_client(pid: u32, out_dir: PathBuf, delay: Option<f64>) {
    let live = match eve_memory::LiveProcess::open(pid) {
        Ok(live) => live,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let clients = discover_clients().unwrap_or_default();
    let window = clients
        .iter()
        .find(|c| c.pid == pid)
        .and_then(|c| c.window);
    let delay = delay.map(|seconds| std::time::Duration::from_secs_f64(seconds.max(0.0)));
    match save_process_sample(&live, window, delay, &out_dir) {
        Ok(path) => println!("sample written: {}", path.display()),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

struct ReadArgs {
    pid: Option<u32>,
    sample: Option<PathBuf>,
    root_address: Option<String>,
    output_file: Option<String>,
    remove_other_dict_entries: bool,
    warmup_iterations: Option<u32>,
}

fn open_reader(pid: Option<u32>, sample: Option<&PathBuf>) -> Result<UiReader, String> {
    match (pid, sample) {
        (Some(pid), _) => UiReader::live(pid).map_err(|e| e.to_string()),
        (None, Some(path)) => UiReader::sample(path).map_err(|e| e.to_string()),
        (None, None) => Err("exactly one of --pid or --sample is required".into()),
    }
}

fn parse_address(text: &str) -> Result<eve_memory::Address, String> {
    let value = text
        .strip_prefix("0x")
        .map(|hex| u64::from_str_radix(hex, 16))
        .unwrap_or_else(|| text.parse::<u64>());
    value
        .map(eve_memory::Address)
        .map_err(|e| format!("invalid address {text:?}: {e}"))
}

fn read_tree(args: ReadArgs) {
    let mut reader = match open_reader(args.pid, args.sample.as_ref()) {
        Ok(reader) => reader,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    if args.remove_other_dict_entries {
        let mut limits = TreeLimits::default();
        limits.keep_other_keys = false;
        reader = reader.with_limits(limits);
    }
    for _ in 0..args.warmup_iterations.unwrap_or(0) {
        if let Err(e) = reader.read_tree() {
            eprintln!("warmup failed: {e}");
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(1111));
    }
    let started = Instant::now();
    let root = match &args.root_address {
        Some(text) => match parse_address(text) {
            Ok(address) => address,
            Err(message) => {
                eprintln!("error: {message}");
                std::process::exit(1);
            }
        },
        None => match reader.find_ui_root() {
            Ok(address) => address,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
    };
    let discovery_elapsed = started.elapsed();
    let tree = match reader.read_tree_at(root) {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let total = started.elapsed();
    let nodes = eve_memory::uitree::count_nodes(&tree);
    println!(
        "root {root}, {nodes} nodes, discovery {discovery_elapsed:.1?}, total {total:.1?}"
    );
    let json = serde_json::to_string_pretty(&tree).expect("tree serialization");
    match args.output_file.as_deref() {
        None | Some("-") => println!("{json}"),
        Some(path) => {
            let path = PathBuf::from(path);
            if let Err(e) = std::fs::write(&path, json) {
                eprintln!("error writing {}: {e}", path.display());
                std::process::exit(1);
            }
            println!("json written: {}", path.display());
        }
    }
}

fn snapshot(pid: Option<u32>, sample: Option<PathBuf>) {
    // Live flavor comes from client discovery; samples default to the
    // 曙光 baseline until archives record their flavor.
    let flavor = match (pid, discover_clients()) {
        (Some(pid), Ok(clients)) => clients
            .iter()
            .find(|client| client.pid == pid)
            .map(|client| client.flavor)
            .unwrap_or(eve_memory::Flavor::Infinity),
        _ => eve_memory::Flavor::Infinity,
    };
    let mut reader = match open_reader(pid, sample.as_ref()) {
        Ok(reader) => reader,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
    };
    let tree = match reader.read_tree() {
        Ok(tree) => tree,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let snapshot = eve_semantics::parse_ui_tree(&tree, flavor);
    println!("{}", serde_json::to_string_pretty(&snapshot).expect("snapshot"));
}
