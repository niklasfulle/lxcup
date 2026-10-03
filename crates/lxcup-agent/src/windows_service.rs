use std::{
    ffi::OsString,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::sync::oneshot;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

const SERVICE_NAME: &str = "lxcup-agent";

define_windows_service!(ffi_service_main, service_main);

pub(super) fn start() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        tracing::error!(?error, "Windows service stopped after an internal error");
    }
}

fn run_service() -> windows_service::Result<()> {
    let (shutdown_sender, shutdown_receiver) = oneshot::channel();
    let shutdown_sender = Arc::new(Mutex::new(Some(shutdown_sender)));
    let event_sender = Arc::clone(&shutdown_sender);
    let status = service_control_handler::register(SERVICE_NAME, move |event| match event {
        ServiceControl::Stop => {
            if let Ok(mut sender) = event_sender.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            }
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;

    status.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    let config = match super::startup_config_from_environment() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(%error, "Windows agent configuration is invalid");
            return set_stopped(&status);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(?error, "could not create Windows agent runtime");
            return set_stopped(&status);
        }
    };

    let result = runtime.block_on(super::run(config, async move {
        let _ = shutdown_receiver.await;
    }));
    if let Err(error) = result {
        tracing::error!(?error, "Windows agent runtime failed");
    }
    status.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })
}

fn set_stopped(
    status: &windows_service::service_control_handler::ServiceStatusHandle,
) -> windows_service::Result<()> {
    status.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(1),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })
}
