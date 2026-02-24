//! Performance benchmarks for approval system
//!
//! Target performance metrics:
//! - Path escalation check: < 10ms
//! - Risk score calculation: < 1ms
//! - Binding validation: < 5ms
//! - Audit log write: < 5ms

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::HashMap;
use std::path::PathBuf;

// Import approval system modules
use alephcore::exec::approval::escalation::{check_path_escalation, is_sensitive_directory};
use alephcore::exec::approval::audit::AuditQuery;
use alephcore::exec::approval::binding::check_binding_compliance;
use alephcore::exec::sandbox::capabilities::{
    Capabilities, FileSystemCapability, NetworkCapability, ProcessCapability,
    EnvironmentCapability,
};

/// Benchmark path escalation check (target: < 10ms)
fn benchmark_path_escalation(c: &mut Criterion) {
    c.bench_function("path_escalation_check_simple", |bencher| {
        let approved_paths = vec!["/tmp/*".to_string(), "/home/user/workspace/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/tmp/output.txt".to_string());

        bencher.iter(|| {
            
            check_path_escalation(
                black_box(&params),
                black_box(&approved_paths),
            )
        });
    });

    c.bench_function("path_escalation_check_complex", |bencher| {
        let approved_paths = vec![
            "/tmp/*".to_string(),
            "/home/user/workspace/**/*.txt".to_string(),
            "/var/log/*.log".to_string(),
            "/opt/data/*".to_string(),
        ];
        let mut params = HashMap::new();
        params.insert("input_file".to_string(), "/home/user/workspace/project/file.txt".to_string());
        params.insert("output_dir".to_string(), "/tmp/results".to_string());
        params.insert("log_path".to_string(), "/var/log/app.log".to_string());

        bencher.iter(|| {
            
            check_path_escalation(
                black_box(&params),
                black_box(&approved_paths),
            )
        });
    });

    c.bench_function("path_escalation_check_sensitive", |bencher| {
        let approved_paths = vec!["/tmp/*".to_string()];
        let mut params = HashMap::new();
        params.insert("file_path".to_string(), "/Users/test/.ssh/id_rsa".to_string());

        bencher.iter(|| {
            
            check_path_escalation(
                black_box(&params),
                black_box(&approved_paths),
            )
        });
    });

    c.bench_function("is_sensitive_directory_check", |bencher| {
        let path = PathBuf::from("/Users/test/.ssh/id_rsa");

        bencher.iter(|| {
            
            is_sensitive_directory(black_box(&path))
        });
    });
}

/// Benchmark risk score calculation (target: < 1ms)
fn benchmark_risk_score(c: &mut Criterion) {
    c.bench_function("risk_score_minimal", |bencher| {
        let caps = Capabilities {
            filesystem: vec![FileSystemCapability::TempWorkspace],
            network: NetworkCapability::Deny,
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        bencher.iter(|| {
            
            AuditQuery::calculate_risk_score(
                black_box(&caps),
                black_box(0),
            )
        });
    });

    c.bench_function("risk_score_moderate", |bencher| {
        let caps = Capabilities {
            filesystem: vec![
                FileSystemCapability::ReadWrite {
                    path: PathBuf::from("/tmp"),
                },
            ],
            network: NetworkCapability::AllowDomains(vec![
                "api.example.com".to_string(),
                "cdn.example.com".to_string(),
            ]),
            process: ProcessCapability {
                no_fork: true,
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        bencher.iter(|| {
            
            AuditQuery::calculate_risk_score(
                black_box(&caps),
                black_box(2),
            )
        });
    });

    c.bench_function("risk_score_high", |bencher| {
        let caps = Capabilities {
            filesystem: vec![
                FileSystemCapability::ReadWrite {
                    path: PathBuf::from("/tmp"),
                },
                FileSystemCapability::ReadWrite {
                    path: PathBuf::from("/home/user"),
                },
            ],
            network: NetworkCapability::AllowAll,
            process: ProcessCapability {
                no_fork: false, // Allow exec
                max_execution_time: 300,
                max_memory_mb: Some(512),
            },
            environment: EnvironmentCapability::Restricted,
        };

        bencher.iter(|| {
            
            AuditQuery::calculate_risk_score(
                black_box(&caps),
                black_box(5),
            )
        });
    });
}

/// Benchmark binding validation (target: < 5ms)
fn benchmark_binding_validation(c: &mut Criterion) {
    c.bench_function("binding_validation_fixed", |bencher| {
        let mut runtime_params = HashMap::new();
        runtime_params.insert("output_file".to_string(), "/tmp/output.txt".to_string());

        let mut declared_bindings = HashMap::new();
        declared_bindings.insert("output_file".to_string(), "/tmp/output.txt".to_string());

        bencher.iter(|| {
            
            check_binding_compliance(
                black_box(&runtime_params),
                black_box(&declared_bindings),
            )
        });
    });

    c.bench_function("binding_validation_pattern", |bencher| {
        let mut runtime_params = HashMap::new();
        runtime_params.insert("input_file".to_string(), "/tmp/data/input.txt".to_string());
        runtime_params.insert("output_file".to_string(), "/tmp/results/output.json".to_string());

        let mut declared_bindings = HashMap::new();
        declared_bindings.insert("input_file".to_string(), "/tmp/**/*.txt".to_string());
        declared_bindings.insert("output_file".to_string(), "/tmp/**/*.json".to_string());

        bencher.iter(|| {
            
            check_binding_compliance(
                black_box(&runtime_params),
                black_box(&declared_bindings),
            )
        });
    });

    c.bench_function("binding_validation_range", |bencher| {
        let mut runtime_params = HashMap::new();
        runtime_params.insert("port".to_string(), "8080".to_string());
        runtime_params.insert("timeout".to_string(), "30".to_string());

        let mut declared_bindings = HashMap::new();
        declared_bindings.insert("port".to_string(), "8000-9000".to_string());
        declared_bindings.insert("timeout".to_string(), "10-60".to_string());

        bencher.iter(|| {
            
            check_binding_compliance(
                black_box(&runtime_params),
                black_box(&declared_bindings),
            )
        });
    });

    c.bench_function("binding_validation_complex", |bencher| {
        let mut runtime_params = HashMap::new();
        runtime_params.insert("input_dir".to_string(), "/tmp/workspace/input".to_string());
        runtime_params.insert("output_dir".to_string(), "/tmp/workspace/output".to_string());
        runtime_params.insert("config_file".to_string(), "/tmp/config.yaml".to_string());
        runtime_params.insert("port".to_string(), "8080".to_string());
        runtime_params.insert("max_workers".to_string(), "4".to_string());

        let mut declared_bindings = HashMap::new();
        declared_bindings.insert("input_dir".to_string(), "/tmp/workspace/*".to_string());
        declared_bindings.insert("output_dir".to_string(), "/tmp/workspace/*".to_string());
        declared_bindings.insert("config_file".to_string(), "/tmp/*.yaml".to_string());
        declared_bindings.insert("port".to_string(), "8000-9000".to_string());
        declared_bindings.insert("max_workers".to_string(), "1-8".to_string());

        bencher.iter(|| {
            
            check_binding_compliance(
                black_box(&runtime_params),
                black_box(&declared_bindings),
            )
        });
    });
}

criterion_group!(
    benches,
    benchmark_path_escalation,
    benchmark_risk_score,
    benchmark_binding_validation,
);
criterion_main!(benches);

