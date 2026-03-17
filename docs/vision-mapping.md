# Vision to Implementation Mapping

> How PulseHive's shared consciousness concepts map to PulseDB storage primitives.
>
> For the full vision discussion, see `discussion.md` in the project root.
> For the PulseDB API details, see `docs/pulsedb-api-reference.md`.

---

## Attractor → Experience

An **attractor** in the shared consciousness model maps to a PulseDB **Experience**:

| Attractor Property | PulseDB Field | Notes |
|---|---|---|
| Position (embedding space location) | `Experience.embedding` | 384-dimensional vector in HNSW index |
| Strength (how established) | `importance × confidence × applications_count` | PulseHive computes composite strength |
| Basin radius (gravitational pull range) | Not stored | PulseHive computes from strength + embedding density |
| Formation history | `source_agent`, `created_at`, `domain` | Who contributed, when, in what domain |
| Temporal decay | `created_at` + `importance` | PulseHive computes decay at query time: `strength × e^(-λ × elapsed)` |

**Key insight**: PulseDB stores the raw data. PulseHive computes the dynamics (basin radius, decay, field gradient) at query time from these primitives.

---

## Field Perception → search_similar()

When an agent "perceives the field," the flow is:

```
1. Agent's query embedding (raw)
       │
       ▼
2. Lens transformation (PulseHive)
   - Warp embedding through lens matrix L
   - Stretches relevant dimensions, compresses irrelevant ones
       │
       ▼
3. search_similar(collective, warped_embedding, k)  (PulseDB)
   - HNSW approximate nearest neighbor search
   - Returns (Experience, similarity_score) tuples
       │
       ▼
4. Post-search re-ranking (PulseHive)
   - Apply domain-specific weight multipliers
   - Factor in temporal decay
   - Factor in attractor strength (importance × confidence)
       │
       ▼
5. Perceived field (what the agent "sees")
```

**Critical boundary**: PulseDB does standard cosine similarity search. The lens math (pre-query warping + post-search re-ranking) lives entirely in PulseHive.

---

## Agent Lens → Pre-query Transformation + SearchFilter

A **lens** reshapes the field from an agent's perspective:

| Lens Operation | Implementation | Layer |
|---|---|---|
| Stretch relevant dimensions | Multiply query embedding by lens matrix | PulseHive (pre-search) |
| Compress irrelevant dimensions | Same matrix transformation | PulseHive (pre-search) |
| Domain filtering | `SearchFilter.domains` | PulseDB (during search) |
| Type filtering | `SearchFilter.experience_types` | PulseDB (during search) |
| Importance threshold | `SearchFilter.min_importance` | PulseDB (during search) |
| Re-rank by domain relevance | Weight multipliers on similarity scores | PulseHive (post-search) |

**Important**: A lens is continuous, not binary. Even with a Safety lens, a sufficiently strong financial attractor can "break through" — the re-ranking reduces its score but doesn't eliminate it. This is by design.

---

## Conflict Detection → Relations

| Concept | PulseDB Primitive |
|---|---|
| Two attractors in tension | Two experiences with `RelationType::Contradicts` between them |
| Reinforcing attractors | Two experiences with `RelationType::Supports` between them |
| Attractor elaboration | `RelationType::Elaborates` (adds detail) |
| Attractor supersession | `RelationType::Supersedes` (replaces) |
| Tension zone detection | Query `get_related()` → find Contradicts relations near query point |

**Flow**: When PulseHive detects two nearby experiences with opposing semantics, it stores a `Contradicts` relation. Future context assembly surfaces these via `ContextCandidates.relations`, making tension visible to perceiving agents.

---

## Attractor Merging → store_insight()

When multiple experiences converge on the same knowledge:

```
Experience A: "Batch ABC-123 has elevated adverse events" (Safety agent)
Experience B: "Batch ABC-123 flagged in financial review" (Finance agent)
          │
          ▼ PulseHive's InsightSynthesizer detects convergence
          │
          ▼
Insight: "Batch ABC-123 multi-signal concern (safety + financial)"
         source_experience_ids: [A, B]
         insight_type: InsightType::CrossDomain
```

The merged insight is stored via `store_insight(NewDerivedInsight { ... })`. It has its own embedding, so it appears in future searches as a stronger, consolidated attractor.

---

## Real-time Propagation → Watch System

```
Agent A records experience → PulseDB emits WatchEvent::Created
                                    │
                                    ▼
                          All subscribed agents receive event
                          via WatchStream (crossbeam channel)
                                    │
                                    ▼
                          Agent B's next field perception
                          already includes the new attractor
```

This is what makes shared consciousness "instant" — agents don't need to poll or be told. The Watch system delivers events with <100ns overhead.

**Subscribe**: `provider.watch(collective_id).await?` returns a `Stream<Item = WatchEvent>`

---

## Context Assembly → get_context_candidates()

The unified retrieval API that assembles the full "field perception" for an agent:

```
ContextRequest {
    collective_id,
    query_embedding,     ← already warped through lens by PulseHive
    max_similar: 20,     ← how many nearby attractors
    max_recent: 10,      ← recent field activity
    include_insights,    ← merged attractors
    include_relations,   ← tension zones & reinforcements
    include_active_agents ← who else is perceiving the field
}
         │
         ▼
ContextCandidates {
    similar_experiences,  ← nearby attractors (sorted by similarity)
    recent_experiences,   ← temporal field state
    insights,             ← consolidated knowledge
    relations,            ← inter-attractor dynamics
    active_agents         ← other agents' presence
}
```

PulseHive then applies post-processing: temporal decay, lens re-ranking, tension zone highlighting, and formats the result as the agent's "perceived field state."

---

## Post-MVP Gaps

These are intentionally deferred. PulseHive handles them at the application layer:

### 1. Temporal Decay
- **Current**: PulseDB stores raw `importance` and `created_at`
- **PulseHive computes**: `effective_strength = importance × e^(-λ × (now - created_at))`
- **Future**: Native PulseDB decay as background process (when optimal λ is known from production data)

### 2. Weighted Search (Full Lens)
- **Current**: PulseHive warps query embedding pre-search (80-90% of lens effect)
- **Limitation**: Can't change attractor positions per-lens
- **Future**: PulseDB multiple index views — same attractors indexed for different lens configs

### 3. Attractor Energy Lifecycle
- **Current**: `applications_count` increments on reinforcement
- **Future**: Dedicated reinforce/decay lifecycle with timestamped events and time-weighted strength
- **Needs**: Real-world usage data to design correctly
