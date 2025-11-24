use serde::Deserialize;
use snafu::prelude::*;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::SystemTime;

#[derive(Deserialize, Debug)]
pub struct DrawioFileConfig {
    pub name: String,
    pub order: Vec<Vec<u8>>,
}

#[derive(Default, Deserialize, Debug)]
//This will generate an error if the json file contains unknown fields.
//This helps to detect typos as they otherwise do no generate an error
#[serde(deny_unknown_fields)]
pub struct DrawioConfig {
    pub individual_configs: Option<Vec<DrawioFileConfig>>,
}

#[derive(Debug, Snafu)]
#[snafu(display("Drawio build error for {output_path:?} : {message}"))]
pub struct DrawioError {
    pub message: String,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub stderr: Vec<u8>,
    pub stdout: Vec<u8>,
    pub exit_code: Option<ExitStatus>,
}

pub struct DrawioExportStep {
    pub output_path: PathBuf,
    pub input_path: PathBuf,
    pub old_modified_time: Option<SystemTime>,
    pub command: Command,
}

impl DrawioExportStep {
    pub fn new(
        output_path: PathBuf,
        input_path: PathBuf,
        old_modified_time: Option<SystemTime>,
        command: Command,
    ) -> Self {
        DrawioExportStep {
            output_path,
            input_path,
            old_modified_time,
            command,
        }
    }

    pub fn spawn(mut self) -> Result<DrawioProcess, DrawioError> {
        let p = DrawioProcess {
            output_path: self.output_path.clone(),
            input_path: self.input_path.clone(),
            old_modified_time: self.old_modified_time,
            handle: self.command.spawn().map_err(|e| DrawioError {
                message: format!("failed to spawn drawio process : {:?}", e).to_string(),
                input_path: self.input_path.clone(),
                output_path: self.output_path.clone(),
                stderr: Vec::new(),
                stdout: Vec::new(),
                exit_code: None,
            })?,
        };
        Ok(p)
    }
}

pub struct DrawioProcess {
    pub output_path: PathBuf,
    pub input_path: PathBuf,
    pub old_modified_time: Option<SystemTime>,
    pub handle: Child,
}

impl DrawioProcess {
    pub fn wait(self) -> Result<(), DrawioError> {
        let output = self.handle.wait_with_output().map_err(|e| DrawioError {
            message: format!("process termination error : {:?}", e).to_string(),
            input_path: self.input_path.clone(),
            output_path: self.output_path.clone(),
            stderr: Vec::new(),
            stdout: Vec::new(),
            exit_code: None,
        })?;
        let mut error_template = DrawioError {
            message: "generic error".to_string(),
            input_path: self.input_path.clone(),
            output_path: self.output_path.clone(),
            stderr: output.stderr,
            stdout: output.stdout,
            exit_code: None,
        };
        if !output.status.success() {
            error_template.message = "error exit code".to_string();
            return Err(error_template);
        }
        match self.old_modified_time {
            Some(old_modified_time) => {
                let new_modified_time = self.output_path.metadata().unwrap().modified().unwrap();
                if old_modified_time.ge(&new_modified_time) {
                    error_template.message = "output file was not updated".to_string();
                    return Err(error_template);
                }
            }
            None => {
                if !self.output_path.exists() {
                    error_template.message = "output file was not created".to_string();
                    return Err(error_template);
                }
            }
        };
        Ok(())
    }
}

pub enum LayerConfig {
    ///Number of layers. Exports [0],[0,1],[0,1,2]...
    Incremental(usize),
    ///For each export step, specify the layers and their order
    /// Example: [ [0,2],[0,1]] will export layers 0,2 in first step
    /// and layers 0,1 in second step
    Custom(Vec<Vec<u8>>),
}

pub struct BuildConfig {
    ///general flags that are passed to drawio. DO NOT pass layer configs here
    pub flags: Vec<String>,
    pub layer_config: LayerConfig,
}
