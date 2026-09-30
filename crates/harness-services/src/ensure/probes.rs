//! The side effects `ensure` needs, behind one trait so tests inject a recorder (bootstrap.py `Launch` /
//! `Healthy` callables plus the daemon mutex probe).

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::daemon::instance::is_daemon_running;
use crate::service_context::ServiceContext;

/// bootstrap.py `http_healthy` timeout.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
/// bootstrap.py `main`: `os.environ.update(...)` with these `.env` keys before launching children.
const FORWARDED_ENV_KEYS: [&str; 2] = ["OTLP_PORT", "OBS_ENDPOINT"];
const HTTP_OK: u16 = 200;

#[cfg(windows)]
const DETACHED_FLAGS: u32 = CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub trait ServiceProbes {
    /// bootstrap.py `launch_detached(command, cwd)`: start and never wait.
    fn launch(&self, command: &[OsString], cwd: &Path) -> io::Result<()>;
    /// bootstrap.py `http_healthy(url)`: HTTP 200 within 2 s.
    fn http_healthy(&self, url: &str) -> bool;
    /// The `serve` daemon's single-instance mutex (lock file off Windows) is held.
    fn daemon_running(&self) -> bool;
}

/// Production probes: real processes, real HTTP, the real daemon mutex.
pub struct SystemProbes {
    context: ServiceContext,
    forwarded_env: Vec<(String, String)>,
}

impl SystemProbes {
    pub fn new(context: &ServiceContext) -> Self {
        let forwarded_env = FORWARDED_ENV_KEYS
            .iter()
            .filter_map(|key| {
                context
                    .env
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect();
        Self {
            context: context.clone(),
            forwarded_env,
        }
    }
}

impl ServiceProbes for SystemProbes {
    fn launch(&self, command: &[OsString], cwd: &Path) -> io::Result<()> {
        let Some((program, args)) = command.split_first() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty launch command",
            ));
        };
        let mut process = Command::new(program);
        process
            .args(args)
            .current_dir(cwd)
            .envs(self.forwarded_env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach(&mut process);
        leave_running(process.spawn()?);
        Ok(())
    }

    fn http_healthy(&self, url: &str) -> bool {
        let agent = ureq::AgentBuilder::new().timeout(HEALTH_TIMEOUT).build();
        matches!(agent.get(url).call(), Ok(response) if response.status() == HTTP_OK)
    }

    fn daemon_running(&self) -> bool {
        is_daemon_running(&self.context)
    }
}

/// The child outlives `ensure` on purpose (Python dropped its `Popen` the same way); never waited.
#[inline]
fn leave_running(_child: Child) {}

#[cfg(windows)]
#[inline]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_FLAGS);
}

#[cfg(not(windows))]
#[inline]
fn detach(_command: &mut Command) {}
