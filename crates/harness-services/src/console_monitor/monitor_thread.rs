//! Body of the dedicated monitor thread (Python `main`).

use super::monitor_log::MonitorLog;
use super::monitor_records::{hook_failed, start_failed, started, stopped, MonitorRecord};
use super::single_instance::{claim, InstanceClaim};
use super::win_event_hook::{install, pump_messages};

/// Mental model: claim the mutex → install hooks → `monitor.start` → pump until quit
/// → unhook → release mutex → `monitor.stop` (emitted on every path after the mutex is held).
pub(super) fn run(log: MonitorLog) {
    let instance = match claim() {
        InstanceClaim::Acquired(guard) => guard,
        InstanceClaim::AlreadyRunning => return,
        InstanceClaim::Unavailable(error_code) => {
            write(&log, &start_failed(error_code));
            return;
        }
    };
    hook_and_pump(&log);
    drop(instance);
    write(&log, &stopped(std::process::id()));
}

/// Installs both hooks and pumps messages; hooks are removed when the guard drops.
fn hook_and_pump(log: &MonitorLog) {
    let hooks = match install(log.clone()) {
        Ok(hooks) => hooks,
        Err(failure) => {
            write(log, &hook_failed(failure.event_id, failure.error_code));
            return;
        }
    };
    write(log, &started(std::process::id()));
    pump_messages();
    drop(hooks);
}

/// The lifecycle log is the only sink; a failed write cannot be reported anywhere else.
#[inline]
fn write(log: &MonitorLog, record: &MonitorRecord) {
    let _ = log.emit(record);
}
