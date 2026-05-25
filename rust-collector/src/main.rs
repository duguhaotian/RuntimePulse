mod collectors;

use chrono::Utc;
use collectors::adapters::command::CommandPlugin;
use collectors::adapters::http::HttpPlugin;
use collectors::core::config::CollectorConfig;
use collectors::core::error::{CollectorError, Result};
use collectors::core::plugin::CollectorPlugin;
use collectors::host_agent::run_host_agent;
use collectors::outlet::batcher::collect_once;
use collectors::outlet::http_ingress::start_local_report_server;
use collectors::outlet::sender::send_local_report;
use collectors::sources::image::cache::ImageCachePlugin;
use collectors::sources::image::download::output_from_event as image_output_from_event;
use collectors::sources::node::cgroupfs::CgroupfsPlugin;
use collectors::sources::node::procfs::ProcfsPlugin;
use collectors::sources::profiling::ebpf_folded::{
    emit_ebpf_folded_profiles, EbpfFoldedProfilePlugin,
};
use collectors::sources::profiling::perf_folded::{
    emit_perf_folded_profiles, PerfFoldedProfilePlugin,
};
use collectors::sources::profiling::report::ProfileReportPlugin;
use collectors::sources::runtime::containerd::{
    collect_containerd_inventory, collect_containerd_task_targets,
    output_from_runtime_event as containerd_output_from_event, stream_containerd_events,
};
use collectors::sources::runtime::containerd_diagnostics::emit_containerd_diagnostics;
use collectors::sources::runtime::cri_diagnostics::emit_crictl_diagnostics;
use collectors::sources::runtime::diagnostics::DiagnosticReportPlugin;
use collectors::sources::runtime::docker::diagnostics::emit_docker_diagnostics;
use collectors::sources::runtime::docker::events::{
    collect_recent_docker_events, empty_output, merge_output, stream_docker_events, DockerEvent,
};
use collectors::sources::runtime::docker::inventory::collect_docker_inventory;
use collectors::sources::runtime::docker::lifecycle::output_from_event as lifecycle_output_from_event;
use collectors::sources::runtime::kubelet::{output_from_cri_event, stream_cri_events};
use collectors::sources::sandbox::cgroupfs::DockerSandboxCgroupfsPlugin;
use collectors::sources::sandbox::firecracker::FirecrackerSandboxPlugin;
use collectors::sources::sandbox::gvisor::GvisorSandboxPlugin;
use collectors::sources::sandbox::kata::KataSandboxPlugin;
use collectors::sources::sandbox::manager::run_docker_sandbox_agent;
use reqwest::blocking::Client;
use serde_json::json;
use std::env;
use std::thread;
use std::time::Instant;

fn main() {
    if env::args().any(|arg| arg == "--version") {
        println!("runtimepulse-collector {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let result = if env::args().any(|arg| arg == "host-procfs") {
        run_host_procfs()
    } else if env::args().any(|arg| arg == "host-agent") {
        run_host_agent_command()
    } else if env::args().any(|arg| arg == "host-cgroupfs") {
        run_host_cgroupfs()
    } else if env::args().any(|arg| arg == "host-docker") {
        run_host_docker()
    } else if env::args().any(|arg| arg == "docker-diagnostics") {
        run_docker_diagnostics()
    } else if env::args().any(|arg| arg == "crictl-diagnostics" || arg == "cri-diagnostics") {
        run_crictl_diagnostics()
    } else if env::args().any(|arg| arg == "containerd-diagnostics") {
        run_containerd_diagnostics()
    } else if env::args().any(|arg| arg == "perf-folded" || arg == "perf-folded-profiles") {
        run_perf_folded_profiles()
    } else if env::args()
        .any(|arg| arg == "ebpf-folded" || arg == "ebpf-folded-profiles" || arg == "off-cpu-folded")
    {
        run_ebpf_folded_profiles()
    } else if env::args().any(|arg| arg == "host-containerd") {
        run_host_containerd()
    } else if env::args().any(|arg| arg == "host-containerd-tasks") {
        run_host_containerd_tasks()
    } else if env::args().any(|arg| arg == "host-containerd-events") {
        run_host_containerd_events()
    } else if env::args().any(|arg| arg == "host-kubelet-events") {
        run_host_kubelet_events()
    } else if env::args().any(|arg| arg == "host-docker-events") {
        run_host_docker_events()
    } else if env::args().any(|arg| arg == "host-docker-cgroupfs") {
        run_host_docker_cgroupfs()
    } else if env::args().any(|arg| arg == "host-docker-sandbox-agent") {
        run_host_docker_sandbox_agent()
    } else {
        run_outlet()
    };

    if let Err(error) = result {
        eprintln!(
            "{}",
            json!({
                "level": "error",
                "message": "collector_failed",
                "error": error.to_string(),
            })
        );
        std::process::exit(1);
    }
}

fn run_outlet() -> Result<()> {
    let config = CollectorConfig::from_env()?;
    let mut plugins = build_plugins(&config)?;
    let client = Client::new();
    let local_reports = start_local_report_server(&config.local_report_addr)?;

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match collect_once(&client, &config, &mut plugins, &local_reports, now) {
            Ok(summary) => {
                if summary.submitted {
                    println!(
                        "{}",
                        json!({
                            "level": "info",
                            "message": "ingest_batch_accepted",
                            "source": summary.source,
                            "localReports": summary.local_reports,
                            "metrics": summary.metrics,
                            "events": summary.events,
                            "traces": summary.traces,
                            "profiles": summary.profiles,
                        })
                    );
                }
            }
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "collector_tick_failed",
                    "error": error.to_string(),
                })
            ),
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }

        if config.once {
            break;
        }
    }

    Ok(())
}

fn run_host_procfs() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    config.plugins = vec!["procfs".to_string()];

    let client = Client::new();
    let mut plugin = ProcfsPlugin::new();

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match plugin.collect(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_procfs_report_accepted",
                        "url": config.local_report_url,
                        "metrics": output.metrics.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_procfs_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_procfs_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }

        if config.once {
            break;
        }
    }

    Ok(())
}

fn run_host_cgroupfs() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    config.plugins = vec!["cgroupfs".to_string()];

    let client = Client::new();
    let mut plugin = CgroupfsPlugin::new(config.cgroup_root.clone(), config.cgroup_max_entries);

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match plugin.collect(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_cgroupfs_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "metrics": output.metrics.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_cgroupfs_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_cgroupfs_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn run_host_docker() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match collect_docker_inventory(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_docker_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "images": output.metadata.images.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_docker_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_docker_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn run_docker_diagnostics() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    emit_docker_diagnostics(&config)
}

fn run_crictl_diagnostics() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    emit_crictl_diagnostics(&config)
}

fn run_containerd_diagnostics() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    emit_containerd_diagnostics(&config)
}

fn run_perf_folded_profiles() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    emit_perf_folded_profiles(&config)
}

fn run_ebpf_folded_profiles() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();
    emit_ebpf_folded_profiles(&config)
}

fn run_host_containerd_tasks() -> Result<()> {
    for target in collect_containerd_task_targets()? {
        println!(
            "{}",
            json!({
                "containerId": target.container_id,
                "namespace": target.namespace,
                "imageRef": target.image_ref,
                "runtimeName": target.runtime_name,
                "pid": target.pid,
                "status": target.status,
                "labels": target.labels,
            })
        );
    }
    Ok(())
}

fn run_host_containerd() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match collect_containerd_inventory(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_containerd_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "images": output.metadata.images.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_containerd_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_containerd_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn run_host_docker_events() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();

    if config.once {
        let now = Utc::now();
        match collect_recent_docker_events(now, &config, docker_event_output) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_docker_events_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "images": output.metadata.images.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_docker_events_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_docker_events_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }
        return Ok(());
    }

    stream_docker_events(&config, |event| {
        let Some(output) = docker_event_output(event, &config)? else {
            return Ok(());
        };
        send_local_report(&client, &config.local_report_url, &output)?;
        println!(
            "{}",
            json!({
                "level": "info",
                "message": "host_docker_event_report_accepted",
                "url": config.local_report_url,
                "sandboxes": output.metadata.sandboxes.len(),
                "images": output.metadata.images.len(),
                "events": output.events.len(),
            })
        );
        Ok(())
    })
}

fn run_host_containerd_events() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();
    stream_containerd_events(&config, |event| {
        let Some(output) = containerd_output_from_event(event, &config)? else {
            return Ok(());
        };
        send_local_report(&client, &config.local_report_url, &output)?;
        println!(
            "{}",
            json!({
                "level": "info",
                "message": "host_containerd_event_report_accepted",
                "url": config.local_report_url,
                "sandboxes": output.metadata.sandboxes.len(),
                "images": output.metadata.images.len(),
                "events": output.events.len(),
            })
        );
        Ok(())
    })
}

fn run_host_kubelet_events() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();
    stream_cri_events(&config, |event| {
        let Some(output) = output_from_cri_event(event, &config) else {
            return Ok(());
        };
        send_local_report(&client, &config.local_report_url, &output)?;
        println!(
            "{}",
            json!({
                "level": "info",
                "message": "host_kubelet_event_report_accepted",
                "url": config.local_report_url,
                "sandboxes": output.metadata.sandboxes.len(),
                "images": output.metadata.images.len(),
                "events": output.events.len(),
            })
        );
        Ok(())
    })
}

fn run_host_docker_cgroupfs() -> Result<()> {
    let mut config = CollectorConfig::from_env()?;
    config.collection_scope = "host".to_string();

    let client = Client::new();
    let mut plugin = DockerSandboxCgroupfsPlugin::new(config.cgroup_root.clone());

    loop {
        let started = Instant::now();
        let now = Utc::now();
        match plugin.collect(now, &config) {
            Ok(output) => match send_local_report(&client, &config.local_report_url, &output) {
                Ok(()) => println!(
                    "{}",
                    json!({
                        "level": "info",
                        "message": "host_docker_cgroupfs_report_accepted",
                        "url": config.local_report_url,
                        "sandboxes": output.metadata.sandboxes.len(),
                        "metrics": output.metrics.len(),
                        "events": output.events.len(),
                    })
                ),
                Err(error) => eprintln!(
                    "{}",
                    json!({
                        "level": "error",
                        "message": "host_docker_cgroupfs_report_failed",
                        "error": error.to_string(),
                    })
                ),
            },
            Err(error) => eprintln!(
                "{}",
                json!({
                    "level": "error",
                    "message": "host_docker_cgroupfs_collect_failed",
                    "error": error.to_string(),
                })
            ),
        }

        if config.once {
            break;
        }

        let elapsed = started.elapsed();
        if config.interval > elapsed {
            thread::sleep(config.interval - elapsed);
        }
    }

    Ok(())
}

fn run_host_docker_sandbox_agent() -> Result<()> {
    run_docker_sandbox_agent(CollectorConfig::from_env()?)
}

fn run_host_agent_command() -> Result<()> {
    run_host_agent(CollectorConfig::from_env()?)
}

fn build_plugins(config: &CollectorConfig) -> Result<Vec<Box<dyn CollectorPlugin>>> {
    let mut plugins: Vec<Box<dyn CollectorPlugin>> = Vec::new();

    for name in &config.plugins {
        match name.as_str() {
            "procfs" => plugins.push(Box::new(ProcfsPlugin::new())),
            "image-cache" | "snapshotter-cache" => plugins.push(Box::new(ImageCachePlugin::new(
                config.image_cache_report_path.clone(),
            ))),
            "profile-report" | "profiles" | "profiling-report" => plugins.push(Box::new(
                ProfileReportPlugin::new(config.profile_report_path.clone()),
            )),
            "perf-folded" | "perf-folded-report" => plugins.push(Box::new(
                PerfFoldedProfilePlugin::new(config.perf_folded_path.clone()),
            )),
            "ebpf-folded" | "ebpf-folded-report" | "off-cpu-folded" => plugins.push(Box::new(
                EbpfFoldedProfilePlugin::new(config.ebpf_folded_path.clone()),
            )),
            "kata" | "kata-report" | "kata-sandbox" => {
                plugins.push(Box::new(KataSandboxPlugin::from_env()))
            }
            "firecracker" | "firecracker-report" | "firecracker-sandbox" => {
                plugins.push(Box::new(FirecrackerSandboxPlugin::from_env()))
            }
            "gvisor" | "gvisor-report" | "gvisor-sandbox" | "runsc" => {
                plugins.push(Box::new(GvisorSandboxPlugin::from_env()))
            }
            "diagnostic-report" | "diagnostics" => {
                plugins.push(Box::new(DiagnosticReportPlugin::new(
                    config.diagnostic_report_path.clone(),
                    config.diagnostic_report_command.clone(),
                    config.diagnostic_report_command_timeout,
                )))
            }
            "command" => {
                if config.command_plugins.is_empty() {
                    return Err(CollectorError::Config(
                        "command plugin requires RUNTIMEPULSE_COMMAND_PLUGIN_CMD or indexed RUNTIMEPULSE_COMMAND_PLUGIN_<N>_CMD".to_string(),
                    ));
                }
                for command in &config.command_plugins {
                    plugins.push(Box::new(CommandPlugin {
                        name: command.name.clone(),
                        command: command.command.clone(),
                        timeout: command.timeout,
                    }));
                }
            }
            "http" => {
                if config.http_plugins.is_empty() {
                    return Err(CollectorError::Config(
                        "http plugin requires RUNTIMEPULSE_HTTP_PLUGIN_URL or indexed RUNTIMEPULSE_HTTP_PLUGIN_<N>_URL".to_string(),
                    ));
                }
                for http in &config.http_plugins {
                    plugins.push(Box::new(HttpPlugin {
                        name: http.name.clone(),
                        url: http.url.clone(),
                        client: Client::new(),
                        timeout: http.timeout,
                    }));
                }
            }
            other => {
                return Err(CollectorError::Config(format!(
                    "unknown collector plugin: {other}"
                )));
            }
        }
    }

    Ok(plugins)
}

fn docker_event_output(
    event: DockerEvent,
    config: &CollectorConfig,
) -> Result<Option<collectors::core::model::PluginOutput>> {
    let mut combined = empty_output(config);
    if let Some(output) = lifecycle_output_from_event(event.clone(), config)? {
        merge_output(&mut combined, output);
    }
    if let Some(output) = image_output_from_event(event, config)? {
        merge_output(&mut combined, output);
    }
    if combined.metadata.sandboxes.is_empty()
        && combined.metadata.images.is_empty()
        && combined.events.is_empty()
    {
        Ok(None)
    } else {
        Ok(Some(combined))
    }
}
