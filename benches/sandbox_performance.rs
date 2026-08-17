use alephcore::sandbox::policy::SandboxPolicy;
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use alephcore::sandbox::{
    build_sandbox, create_platform_driver_from_config, SandboxCapabilities, SandboxConfig,
};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::path::PathBuf;
use std::sync::Arc;

fn benchmark_policy_generation(c: &mut Criterion) {
    c.bench_function("policy_generation_minimal", |b| {
        let caps = SandboxCapabilities::default();
        b.iter(|| {
            let policy: SandboxPolicy = (&caps).into();
            black_box(policy);
        });
    });

    c.bench_function("policy_generation_complex", |b| {
        let caps = SandboxCapabilities {
            fs_read: vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/usr/lib"),
                PathBuf::from("/etc"),
            ],
            fs_write: vec![PathBuf::from("/tmp/workspace/output")],
            ..Default::default()
        };
        b.iter(|| {
            let policy: SandboxPolicy = (&caps).into();
            black_box(policy);
        });
    });
}

fn benchmark_driver_creation(c: &mut Criterion) {
    c.bench_function("platform_driver_creation", |b| {
        let config = SandboxConfig::default();
        b.iter(|| {
            let driver = create_platform_driver_from_config(&config);
            black_box(driver);
        });
    });
}

fn benchmark_sandbox_assembly(c: &mut Criterion) {
    c.bench_function("sandbox_assembly_enabled", |b| {
        let config = SandboxConfig {
            workspace_root: PathBuf::from("/tmp/aleph-workspace"),
            enabled: true,
            default_timeout_seconds: 60,
            max_output_bytes: 1024 * 1024,
            ..Default::default()
        };
        let driver = create_platform_driver_from_config(&config);
        let approval = Arc::new(alephcore::sandbox::exec_approval::gate::ApprovalGate::new(
            None,
        ));
        let shell_security = alephcore::ShellSecurityConfig::default();

        b.iter(|| {
            let sandbox = build_sandbox(
                &config,
                driver.clone(),
                approval.clone(),
                SandboxRateLimitConfig::default(),
                &shell_security,
            );
            black_box(sandbox);
        });
    });

    c.bench_function("sandbox_assembly_disabled", |b| {
        let config = SandboxConfig {
            workspace_root: PathBuf::from("/tmp/aleph-workspace"),
            enabled: false,
            default_timeout_seconds: 60,
            max_output_bytes: 1024 * 1024,
            ..Default::default()
        };
        let driver = create_platform_driver_from_config(&config);
        let approval = Arc::new(alephcore::sandbox::exec_approval::gate::ApprovalGate::new(
            None,
        ));
        let shell_security = alephcore::ShellSecurityConfig::default();

        b.iter(|| {
            let sandbox = build_sandbox(
                &config,
                driver.clone(),
                approval.clone(),
                SandboxRateLimitConfig::default(),
                &shell_security,
            );
            black_box(sandbox);
        });
    });
}

fn benchmark_capability_check(c: &mut Criterion) {
    use alephcore::sandbox::capabilities::NetworkPolicy;

    c.bench_function("capability_check_strict", |b| {
        let caps = SandboxCapabilities::default();
        b.iter(|| {
            let needs_approval = caps.network != NetworkPolicy::None
                || caps.spawn_subprocess
                || !caps.fs_write.is_empty();
            black_box(needs_approval);
        });
    });

    c.bench_function("capability_check_permissive", |b| {
        let caps = SandboxCapabilities {
            network: NetworkPolicy::AllowAll,
            spawn_subprocess: true,
            fs_write: vec![PathBuf::from("/tmp/output")],
            ..Default::default()
        };
        b.iter(|| {
            let needs_approval = caps.network != NetworkPolicy::None
                || caps.spawn_subprocess
                || !caps.fs_write.is_empty();
            black_box(needs_approval);
        });
    });
}

criterion_group!(
    benches,
    benchmark_policy_generation,
    benchmark_driver_creation,
    benchmark_sandbox_assembly,
    benchmark_capability_check
);
criterion_main!(benches);
