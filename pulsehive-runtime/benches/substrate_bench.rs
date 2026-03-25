//! Performance benchmarks for substrate operations.
//!
//! Run with: `cargo bench -p pulsehive-runtime`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pulsedb::{
    AgentId, CollectiveId, Config, ExperienceType, NewExperience, PulseDB, PulseDBSubstrate,
    SubstrateProvider,
};
use std::sync::Arc;

fn create_substrate(path: &std::path::Path) -> (Arc<PulseDBSubstrate>, CollectiveId) {
    let db = PulseDB::open(path, Config::with_builtin_embeddings()).unwrap();
    let cid = db.create_collective("bench").unwrap();
    let substrate = Arc::new(PulseDBSubstrate::from_db(db));
    (substrate, cid)
}

fn seed_experiences(
    rt: &tokio::runtime::Runtime,
    substrate: &dyn SubstrateProvider,
    cid: CollectiveId,
    count: usize,
) {
    rt.block_on(async {
        for i in 0..count {
            let exp = NewExperience {
                collective_id: cid,
                content: format!("Experience number {i}: This is a test experience with some content about topic {}", i % 10),
                experience_type: ExperienceType::Generic { category: Some(format!("category-{}", i % 5)) },
                embedding: None,
                importance: 0.5 + (i as f32 % 5.0) * 0.1,
                confidence: 0.7,
                domain: vec![format!("domain-{}", i % 3)],
                source_agent: AgentId("bench-agent".into()),
                source_task: None,
                related_files: vec![],
            };
            substrate.store_experience(exp).await.unwrap();
        }
    });
}

fn bench_store_experience(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (substrate, cid) = create_substrate(&dir.path().join("store.db"));

    let mut counter = 0u64;
    c.bench_function("store_experience", |b| {
        b.iter(|| {
            counter += 1;
            rt.block_on(async {
                let exp = NewExperience {
                    collective_id: cid,
                    content: format!("Bench experience {counter}"),
                    experience_type: ExperienceType::Generic { category: None },
                    embedding: None,
                    importance: 0.5,
                    confidence: 0.5,
                    domain: vec![],
                    source_agent: AgentId("bench".into()),
                    source_task: None,
                    related_files: vec![],
                };
                substrate.store_experience(exp).await.unwrap();
            });
        });
    });
}

fn bench_get_recent(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("get_recent");

    for size in [100, 500, 1000, 10_000] {
        let dir = tempfile::tempdir().unwrap();
        let (substrate, cid) = create_substrate(&dir.path().join("recent.db"));
        seed_experiences(&rt, substrate.as_ref(), cid, size);

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    substrate.get_recent(cid, 20).await.unwrap();
                });
            });
        });
    }
    group.finish();
}

fn bench_search_similar(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("search_similar");

    for size in [100, 500, 1000, 10_000] {
        let dir = tempfile::tempdir().unwrap();
        let (substrate, cid) = create_substrate(&dir.path().join("search.db"));
        seed_experiences(&rt, substrate.as_ref(), cid, size);

        // Use a random-ish embedding for querying
        let query_embedding: Vec<f32> = (0..384).map(|i| (i as f32 * 0.01).sin()).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    substrate
                        .search_similar(cid, &query_embedding, 20)
                        .await
                        .unwrap();
                });
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_store_experience,
    bench_get_recent,
    bench_search_similar
);
criterion_main!(benches);
