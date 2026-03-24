//! Performance benchmarks for field dynamics and perception re-ranking.
//!
//! Run with: `cargo bench -p pulsehive-runtime`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pulsedb::{AgentId, CollectiveId, Experience, ExperienceId, ExperienceType, Timestamp};
use pulsehive_core::lens::Lens;
use pulsehive_runtime::field::{cosine_distance, AttractorConfig, AttractorDynamics};
use pulsehive_runtime::perception::rerank;

fn mock_experience(idx: usize) -> Experience {
    // Generate a pseudo-random embedding based on index
    let embedding: Vec<f32> = (0..384)
        .map(|d| ((idx as f32 * 0.37 + d as f32 * 0.13).sin() + 1.0) / 2.0)
        .collect();

    Experience {
        id: ExperienceId::new(),
        collective_id: CollectiveId::new(),
        content: format!("Experience {idx} about topic {}", idx % 10),
        experience_type: ExperienceType::Generic {
            category: Some(format!("cat-{}", idx % 5)),
        },
        embedding,
        importance: 0.3 + (idx as f32 % 7.0) * 0.1,
        confidence: 0.5 + (idx as f32 % 5.0) * 0.1,
        applications: (idx % 3) as u32,
        domain: vec![format!("domain-{}", idx % 3)],
        source_agent: AgentId("bench-agent".into()),
        source_task: None,
        related_files: vec![],
        timestamp: Timestamp::now(),
        archived: false,
    }
}

fn bench_cosine_distance(c: &mut Criterion) {
    let a: Vec<f32> = (0..384).map(|i| (i as f32 * 0.01).sin()).collect();
    let b: Vec<f32> = (0..384).map(|i| (i as f32 * 0.02).cos()).collect();

    c.bench_function("cosine_distance_384d", |bench| {
        bench.iter(|| cosine_distance(&a, &b));
    });
}

fn bench_influence_at(c: &mut Criterion) {
    let config = AttractorConfig::default();
    let exp = mock_experience(42);
    let attractor = AttractorDynamics::from_experience(&exp, &config);
    let query: Vec<f32> = (0..384).map(|i| (i as f32 * 0.03).sin()).collect();

    c.bench_function("influence_at_384d", |bench| {
        bench.iter(|| attractor.influence_at(&query, &exp.embedding));
    });
}

fn bench_rerank_without_attractors(c: &mut Criterion) {
    let lens = Lens::new(["domain-0", "domain-1"]);
    let mut group = c.benchmark_group("rerank_no_attractors");

    for size in [100, 500, 1000] {
        let experiences: Vec<Experience> = (0..size).map(mock_experience).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let exps = experiences.clone();
                rerank(exps, &lens, None)
            });
        });
    }
    group.finish();
}

fn bench_rerank_with_attractors(c: &mut Criterion) {
    let lens = Lens::new(["domain-0", "domain-1"]);
    let config = AttractorConfig::default();
    let mut group = c.benchmark_group("rerank_with_attractors");

    for size in [100, 500] {
        let experiences: Vec<Experience> = (0..size).map(mock_experience).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                let exps = experiences.clone();
                rerank(exps, &lens, Some(&config))
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_cosine_distance,
    bench_influence_at,
    bench_rerank_without_attractors,
    bench_rerank_with_attractors,
);
criterion_main!(benches);
