mod drawio;

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::{prelude::*, ThreadPoolBuilder};
use regex::Regex;
use snafu::prelude::*;
use std::collections::HashMap;
use std::env;
use std::fs::{self, create_dir_all, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Snafu)]
enum AppError {
    ///catch-all error type
    #[snafu(whatever, display("{message}"))]
    Whatever {
        message: String,
        #[snafu(source(from(Box<dyn std::error::Error>, Some)))]
        source: Option<Box<dyn std::error::Error>>,
    },
}

#[derive(Parser)]
#[command(version,about,long_about=None)]
struct Args {
    ///Path to folder with input files
    #[arg(short, long, default_value = "./")]
    input: String,

    ///Path to folder where output gets stored.
    /// Will be created if it does not exist
    #[arg(short, long, default_value = "./out")]
    output: String,

    ///Path to drawio binary. Defaults to "drawio"
    #[arg(long)]
    drawio: Option<String>,

    ///If true, use lower resolution for faster latex build times
    #[arg(long, default_value = "false")]
    draft: bool,

    ///Drawio build args. Separate flags and flag value with whitespaces.
    ///Don't forget to put the whole thing in quotes
    #[arg(long, default_value = "-x -f png -t -s 5")]
    build_args: String,

    ///Path to optional config file
    #[arg(long)]
    config: Option<String>,

    /// Max number of parallel drawio exports. We automatically estimate a sensible default
    /// but systems with a lower CPU to memory ratio might need a lower value
    #[arg(long)]
    jobs: Option<usize>,
}

/// Convert LayerConfig to strings that can be passed to the drawio cli
fn assemble_layer_cli_flag(config: &drawio::LayerConfig) -> Vec<String> {
    let mut result = Vec::new();
    match config {
        drawio::LayerConfig::Incremental(layer_count) => {
            let mut buf: String = "0".to_string();
            result.push(buf.clone());
            for layer in 1..*layer_count {
                buf += &format!(",{}", layer);
                result.push(buf.clone());
            }
            result
        }
        drawio::LayerConfig::Custom(v) => v
            .iter()
            .map(|inner| {
                inner
                    .iter()
                    .map(|num| format!("{}", num))
                    .collect::<Vec<String>>()
                    .join(",")
            })
            .collect(),
    }
}

/// Create a invokable Command for each export step
fn create_job(
    drawio_binary: &str,
    file: &PathBuf,
    config: &drawio::BuildConfig,
    out_dir: &str,
) -> Result<Vec<drawio::DrawioExportStep>, AppError> {
    // Build the command
    let file_name = file.file_stem().unwrap().to_str().unwrap();
    let full_file_path = file.as_path().as_os_str().to_str().unwrap();

    let mut jobs = Vec::new();
    let export_steps = assemble_layer_cli_flag(&config.layer_config);
    // Add the file and flags to the command
    for (idx, step) in export_steps.iter().enumerate() {
        let mut command = Command::new(drawio_binary);

        let output_path = Path::new(out_dir).join(format!("{}-{}.png", file_name, idx));

        //skip build if output file is older than input file, i.e. no changes since built
        let mut old_modified_time = None;
        if output_path.exists() {
            let out_modified = output_path.metadata().unwrap().modified().unwrap();
            let in_modified = file.metadata().unwrap().modified().unwrap();
            if out_modified.ge(&in_modified) {
                continue;
            }
            old_modified_time = Some(out_modified);
        }

        command.args(&config.flags).arg("-o").arg(&output_path);
        command.arg("--layers");
        command.arg(&step);

        command.arg(full_file_path);
        command.stdout(Stdio::piped());
        command.stderr(Stdio::piped());
        command.current_dir(
            env::current_dir()
                .map_err(|e| drawio::DrawioError {
                    message: format!("failed to spawn drawio process : {:?}", e).to_string(),
                    input_path: PathBuf::from(full_file_path),
                    output_path: output_path.clone(),
                    stderr: Vec::new(),
                    stdout: Vec::new(),
                    exit_code: None,
                })
                .map_err(|e| AppError::Whatever {
                    message: e.message,
                    source: None,
                })?,
        );

        jobs.push(drawio::DrawioExportStep::new(
            output_path.clone(),
            PathBuf::from(full_file_path),
            old_modified_time,
            command,
        ));
    }

    Ok(jobs)
}

/// Check well known locations and `hint` for drawio binary. Hint is preferred
/// Returns first matching path
fn search_drawio_binary(hint: Option<String>) -> Option<String> {
    let mut candidates = vec![
        //macos
        "/Applications/draw.io.app/Contents/MacOS//draw.io".to_string(),
        //generic 1
        "drawio".to_string(),
        //generic 2
        "draw.io".to_string(),
    ];

    //insert at front is important so that hint is preferred over the other paths
    if let Some(hint) = hint {
        candidates.insert(0, hint);
    }

    for c in &candidates {
        match Command::new(c).arg("--version").output() {
            Ok(_) => return Some(c.to_string()),
            Err(_) => (),
        }
    }

    None
}

fn main() -> Result<(), AppError> {
    let args = Args::parse();

    let mut drawio_flags: Vec<String> = args.build_args.split(" ").map(|v| v.to_string()).collect();

    //If draft mode, change scale to 1
    if args.draft {
        let mut scale_flag_idx = None;
        for (i, v) in drawio_flags.iter().enumerate() {
            if v == "-s" || v == "--scale" {
                scale_flag_idx = Some(i);
                break;
            }
        }
        if let Some(idx) = scale_flag_idx {
            if drawio_flags.len() < idx + 1 {
                whatever!("Scale flag does not have argument");
            }
            drawio_flags[idx + 1] = "1".to_string();
        }
    }

    let config: drawio::DrawioConfig = match args.config {
        Some(path) => {
            serde_json::from_reader(File::open(&path).whatever_context::<String, AppError>(
                format!("Failed to open config file {}", path),
            )?)
            .whatever_context::<&str, AppError>("Failed to parse config file")?
        }
        None => drawio::DrawioConfig::default(),
    };

    //Later we need to quickly check if there is a config override for a given file
    let mut file_to_config: HashMap<String, &drawio::DrawioFileConfig> = HashMap::new();
    if let Some(overrides) = &config.individual_configs {
        for x in overrides {
            file_to_config.insert(x.name.clone(), x);
        }
    }

    let drawio_path = match search_drawio_binary(args.drawio) {
        Some(v) => v,
        None => whatever!(
            "Failed to locate drawio binary. Please specify path with \"--drawio\" cli argument"
        ),
    };

    create_dir_all(&args.output).whatever_context::<std::string::String, AppError>(format!(
        "Failed to create output dir at {}",
        &args.output
    ))?;

    let mut drawio_files = Vec::new();
    let layer_re = Regex::new(r#"<mxCell id=".*" value=".*" parent="." />"#)
        .whatever_context::<std::string::String, AppError>(
            "failed to compile layer extraction regexp".to_string(),
        )?;
    for dir_entry in fs::read_dir(&args.input).whatever_context::<std::string::String, AppError>(
        format!("error listing files in folder {}", &args.input),
    )? {
        let dir_entry =
            dir_entry.whatever_context::<std::string::String, AppError>("".to_string())?;
        if !dir_entry.path().is_file() {
            continue;
        }
        match dir_entry.path().extension() {
            Some(v) => {
                if v != "drawio" {
                    continue;
                }
            }
            None => continue,
        }

        let content = fs::read_to_string(&dir_entry.path())
            .whatever_context::<std::string::String, AppError>(format!(
                "failed to read file {:?}",
                &dir_entry.path()
            ))?;

        let layer_count = match layer_re.find_iter(&content).count() {
            0 => 1,
            v => v,
        };

        drawio_files.push((dir_entry.path(), layer_count));
    }

    let mut jobs = Vec::new();

    // Parse config and create runnable command for each export step
    for (input_path, layer_count) in &drawio_files {
        let file_name = input_path
            .file_name()
            .expect(&format!(
                "unexpected malformed path {:?}. Should no longer happen at this stage",
                input_path
            ))
            .to_str()
            .unwrap()
            .to_string();

        let config = match file_to_config.get(&file_name) {
            Some(custom_config) => drawio::BuildConfig {
                flags: drawio_flags.clone(),
                layer_config: drawio::LayerConfig::Custom(custom_config.order.clone()),
            },
            None => drawio::BuildConfig {
                flags: drawio_flags.clone(),
                layer_config: drawio::LayerConfig::Incremental(*layer_count),
            },
        };
        let local_jobs = create_job(&drawio_path, input_path, &config, &args.output)?;

        jobs.extend(local_jobs);
    }

    let task_count: usize = drawio_files.iter().map(|(_, steps)| *steps).sum();
    let progress_bar = ProgressBar::new(task_count as u64);
    progress_bar.set_style(
        ProgressStyle::with_template("[{elapsed}] {wide_bar} {pos:>7}/{len:7} {msg}")
            .expect("progress bar template failed"),
    );
    progress_bar.enable_steady_tick(Duration::from_millis(200));
    progress_bar.inc(0);

    if let Some(jobs) = args.jobs {
        ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build_global()
            .whatever_context::<String, AppError>(format!(
                "Failed to create Threadpool that limits to {} threads",
                jobs
            ))?;
    }

    // Run each export step
    let first_err = jobs.into_par_iter().try_for_each(|command| {
        let res = command.spawn()?.wait();
        progress_bar.inc(1);
        res
    });

    match first_err {
        Ok(_) => progress_bar.finish_with_message("Built all figures"),
        Err(e) => {
            let log_path = PathBuf::from(&args.output).join("drawio-builder-errors.log");
            let mut log_file = File::create(&log_path)
                .whatever_context::<String, AppError>(format!(
                "At least one figure failed to build and we failed to create the error log at {:?}",
                log_path
            ))?;
            write!(
                log_file,
                "Stderr and Stdout when trying to create {:?}\n\n",
                &e.output_path
            )
            .whatever_context::<&str, AppError>(
                "Failed to write failed figure's build to log file",
            )?;
            log_file
                .write_all(&e.stdout)
                .whatever_context::<&str, AppError>(
                    "Failed to write stdout of failed figure's build to log file",
                )?;
            log_file
                .write_all(&e.stderr)
                .whatever_context::<&str, AppError>(
                    "Failed to write stderr or failed figure's build to log file",
                )?;
            whatever!(
                "At least one figure failed to build. Error log has been created at {:?}",
                &log_path
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    fn test_assemble_layer_flag_incremental() {
        let want = vec!["0".to_string()];
        let got = assemble_layer_cli_flag(&drawio::LayerConfig::Incremental(1));
        assert_eq!(want, got);
        let want = vec!["0".to_string(), "0,1".to_string(), "0,1,2".to_string()];
        let got = assemble_layer_cli_flag(&drawio::LayerConfig::Incremental(3));
        assert_eq!(want, got);
    }

    #[test]
    fn test_assemble_layer_flag_custom() {
        let want = vec!["1,0".to_string(), "2,5".to_string()];
        let got = assemble_layer_cli_flag(&drawio::LayerConfig::Custom(vec![
            vec![1, 0],
            vec![2, 5],
        ]));
        assert_eq!(want, got);
    }
}
