//! Runs whisper.cpp and llama.cpp as local helper servers on 127.0.0.1.
//! Models stay loaded in memory, so each dictation only pays for inference.
//! Children are tied to a Windows job object, so they die with the app.
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::hardware::engine_threads;

struct Server {
    child: Child,
    port: u16,
    model: String,
}

pub struct Engines {
    res: PathBuf,
    whisper: Mutex<Option<Server>>,
    llama: Mutex<Option<Server>>,
    http: reqwest::blocking::Client,
    #[cfg(windows)]
    job: usize,
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(48123)
}

impl Engines {
    pub fn new(res: PathBuf) -> Self {
        let http = reqwest::blocking::Client::builder().no_proxy().build().expect("http client");
        Self {
            res,
            whisper: Mutex::new(None),
            llama: Mutex::new(None),
            http,
            #[cfg(windows)]
            job: job::create(),
        }
    }

    fn spawn(&self, exe: &Path, args: &[String]) -> Result<Child, String> {
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .current_dir(exe.parent().unwrap_or(Path::new(".")))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let child = cmd.spawn().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
        #[cfg(windows)]
        job::assign(self.job, &child);
        Ok(child)
    }

    fn wait_ready(&self, child: &mut Child, url: &str, timeout: Duration) -> Result<(), String> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("engine exited early ({status})"));
            }
            if let Ok(r) = self.http.get(url).timeout(Duration::from_millis(800)).send() {
                if r.status().is_success() {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err("engine did not become ready in time".into())
    }

    fn alive(slot: &mut Option<Server>, model: &str) -> Option<u16> {
        let s = slot.as_mut()?;
        (s.model == model && matches!(s.child.try_wait(), Ok(None))).then_some(s.port)
    }

    fn stop(slot: &mut Option<Server>) {
        if let Some(mut s) = slot.take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }

    /// Start (or reuse) whisper-server with the given model file. Returns its port.
    pub fn ensure_whisper(&self, model: &str) -> Result<u16, String> {
        let mut slot = self.whisper.lock().unwrap();
        if let Some(p) = Self::alive(&mut slot, model) {
            return Ok(p);
        }
        Self::stop(&mut slot);
        let exe = self.res.join("bin/whisper/whisper-server.exe");
        let model_path = self.res.join("models").join(model);
        if !model_path.exists() {
            return Err(format!("speech model missing: {}", model_path.display()));
        }
        let port = free_port();
        let args: Vec<String> = vec![
            "-m".into(), model_path.to_string_lossy().into(),
            "--host".into(), "127.0.0.1".into(),
            "--port".into(), port.to_string(),
            "-t".into(), engine_threads().to_string(),
            "-l".into(), "en".into(),
            "-nt".into(),
            "-bo".into(), "1".into(),
        ];
        let started = Instant::now();
        let mut child = self.spawn(&exe, &args)?;
        self.wait_ready(&mut child, &format!("http://127.0.0.1:{port}/"), Duration::from_secs(60))?;
        crate::log!("whisper ready ({model}) on :{port} in {:?}", started.elapsed());
        *slot = Some(Server { child, port, model: model.into() });
        Ok(port)
    }

    pub fn transcribe(&self, model: &str, wav: Vec<u8>, prompt: &str) -> Result<String, String> {
        let port = self.ensure_whisper(model)?;
        let part = reqwest::blocking::multipart::Part::bytes(wav)
            .file_name("speech.wav")
            .mime_str("audio/wav")
            .map_err(|e| e.to_string())?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", part)
            .text("response_format", "json")
            .text("temperature", "0.0");
        if !prompt.is_empty() {
            form = form.text("prompt", prompt.to_string());
        }
        let resp = self
            .http
            .post(format!("http://127.0.0.1:{port}/inference"))
            .multipart(form)
            .timeout(Duration::from_secs(120))
            .send()
            .map_err(|e| format!("speech engine request failed: {e}"))?;
        let v: serde_json::Value = resp.json().map_err(|e| format!("bad speech response: {e}"))?;
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return Err(err.to_string());
        }
        Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string())
    }

    pub const LLM_MODEL: &'static str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";

    pub fn ensure_llama(&self) -> Result<u16, String> {
        let mut slot = self.llama.lock().unwrap();
        if let Some(p) = Self::alive(&mut slot, Self::LLM_MODEL) {
            return Ok(p);
        }
        Self::stop(&mut slot);
        let exe = self.res.join("bin/llama/llama-server.exe");
        let model_path = self.res.join("models").join(Self::LLM_MODEL);
        if !model_path.exists() {
            return Err("cleanup model missing".into());
        }
        let port = free_port();
        let args: Vec<String> = vec![
            "-m".into(), model_path.to_string_lossy().into(),
            "--host".into(), "127.0.0.1".into(),
            "--port".into(), port.to_string(),
            "-t".into(), engine_threads().to_string(),
            "-c".into(), "2048".into(),
            "--no-webui".into(),
        ];
        let started = Instant::now();
        let mut child = self.spawn(&exe, &args)?;
        self.wait_ready(&mut child, &format!("http://127.0.0.1:{port}/health"), Duration::from_secs(90))?;
        crate::log!("llama ready on :{port} in {:?}", started.elapsed());
        *slot = Some(Server { child, port, model: Self::LLM_MODEL.into() });
        Ok(port)
    }

    /// True when the cleanup model is already loaded (we never wait for it mid-dictation).
    pub fn llama_ready(&self) -> Option<u16> {
        let mut slot = self.llama.lock().ok()?;
        Self::alive(&mut slot, Self::LLM_MODEL)
    }

    pub fn chat(&self, port: u16, messages: serde_json::Value, max_tokens: u32, timeout: Duration) -> Result<String, String> {
        let body = serde_json::json!({
            "messages": messages,
            "temperature": 0.1,
            "max_tokens": max_tokens,
            "stream": false
        });
        let resp = self
            .http
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .json(&body)
            .timeout(timeout)
            .send()
            .map_err(|e| e.to_string())?;
        let v: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "no content in reply".into())
    }

    pub fn stop_llama(&self) {
        Self::stop(&mut self.llama.lock().unwrap());
    }

    pub fn shutdown(&self) {
        Self::stop(&mut self.whisper.lock().unwrap());
        self.stop_llama();
    }
}

#[cfg(windows)]
mod job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::System::JobObjects::*;

    pub fn create() -> usize {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            job as usize
        }
    }

    pub fn assign(job: usize, child: &Child) {
        if job != 0 {
            unsafe {
                AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
            }
        }
    }
}
