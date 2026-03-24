//! Python bindings for LlmConfig, Lens, and RecencyCurve.

use std::collections::HashMap;

use pulsehive_core::lens::{ExperienceTypeTag, Lens, RecencyCurve};
use pulsehive_core::llm::LlmConfig;
use pyo3::prelude::*;

// ── LlmConfig ────────────────────────────────────────────────────────

/// LLM model selection and generation parameters.
///
/// Args:
///     provider: Provider name matching HiveMind builder key (e.g., "openai", "anthropic")
///     model: Model identifier (e.g., "gpt-4", "claude-sonnet-4-6")
///     temperature: Sampling temperature (0.0 = deterministic, 1.0+ = creative). Default: 0.7
///     max_tokens: Maximum tokens to generate. Default: 4096
#[pyclass(name = "LlmConfig", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyLlmConfig {
    pub(crate) inner: LlmConfig,
}

#[pymethods]
impl PyLlmConfig {
    #[new]
    #[pyo3(signature = (provider, model, temperature=0.7, max_tokens=4096))]
    fn new(provider: String, model: String, temperature: f32, max_tokens: u32) -> Self {
        Self {
            inner: LlmConfig {
                provider,
                model,
                temperature,
                max_tokens,
            },
        }
    }

    /// Provider name.
    #[getter]
    fn provider(&self) -> &str {
        &self.inner.provider
    }

    /// Model identifier.
    #[getter]
    fn model(&self) -> &str {
        &self.inner.model
    }

    /// Sampling temperature.
    #[getter]
    fn temperature(&self) -> f32 {
        self.inner.temperature
    }

    /// Maximum tokens to generate.
    #[getter]
    fn max_tokens(&self) -> u32 {
        self.inner.max_tokens
    }

    fn __repr__(&self) -> String {
        format!(
            "LlmConfig(provider='{}', model='{}', temperature={}, max_tokens={})",
            self.inner.provider, self.inner.model, self.inner.temperature, self.inner.max_tokens
        )
    }
}

// ── RecencyCurve ─────────────────────────────────────────────────────

/// Time decay function controlling how recency affects perception.
///
/// Use static methods to create:
///     RecencyCurve.exponential(72.0)  # 72-hour half-life
///     RecencyCurve.uniform()          # no decay
#[pyclass(name = "RecencyCurve", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyRecencyCurve {
    pub(crate) inner: RecencyCurve,
}

#[pymethods]
impl PyRecencyCurve {
    /// Create exponential decay with the given half-life in hours.
    ///
    /// Formula: weight = 0.5^(age_hours / half_life_hours)
    #[staticmethod]
    fn exponential(half_life_hours: f32) -> Self {
        Self {
            inner: RecencyCurve::Exponential { half_life_hours },
        }
    }

    /// Create uniform weighting (no temporal decay).
    #[staticmethod]
    fn uniform() -> Self {
        Self {
            inner: RecencyCurve::Uniform,
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            RecencyCurve::Exponential { half_life_hours } => {
                format!("RecencyCurve.exponential({half_life_hours})")
            }
            RecencyCurve::Uniform => "RecencyCurve.uniform()".to_string(),
        }
    }
}

// ── Lens ─────────────────────────────────────────────────────────────

/// Perception filter that shapes how an agent sees the substrate.
///
/// Different agents can have different lenses, causing them to perceive the
/// same shared substrate differently based on domain focus, type weights,
/// and recency preferences.
///
/// Args:
///     domains: List of domain focus strings (e.g., ["safety", "clinical"])
///     attention_budget: Max experiences to perceive per cycle. Default: 50
///     recency_curve: Temporal decay function. Default: exponential(72.0)
///     type_weights: Dict mapping type names to weights. Default: all 1.0
#[pyclass(name = "Lens", frozen, from_py_object)]
#[derive(Clone)]
pub struct PyLens {
    pub(crate) inner: Lens,
}

#[pymethods]
impl PyLens {
    #[new]
    #[pyo3(signature = (domains, attention_budget=50, recency_curve=None, type_weights=None))]
    fn new(
        domains: Vec<String>,
        attention_budget: usize,
        recency_curve: Option<PyRecencyCurve>,
        type_weights: Option<HashMap<String, f32>>,
    ) -> Self {
        let mut lens = Lens::new(domains);
        lens.attention_budget = attention_budget;
        if let Some(rc) = recency_curve {
            lens.recency_curve = rc.inner;
        }
        if let Some(weights) = type_weights {
            for (key, value) in weights {
                if let Some(tag) = parse_experience_type_tag(&key) {
                    lens.type_weights.insert(tag, value);
                }
            }
        }
        Self { inner: lens }
    }

    /// Domain focus strings.
    #[getter]
    fn domain_focus(&self) -> Vec<String> {
        self.inner.domain_focus.clone()
    }

    /// Maximum experiences to perceive per cycle.
    #[getter]
    fn attention_budget(&self) -> usize {
        self.inner.attention_budget
    }

    /// Recency curve configuration.
    #[getter]
    fn recency_curve(&self) -> PyRecencyCurve {
        PyRecencyCurve {
            inner: self.inner.recency_curve.clone(),
        }
    }

    /// Type weights as dict.
    #[getter]
    fn type_weights(&self) -> HashMap<String, f32> {
        self.inner
            .type_weights
            .iter()
            .map(|(k, v)| (format!("{k:?}").to_lowercase(), *v))
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "Lens(domains={:?}, attention_budget={}, recency_curve={})",
            self.inner.domain_focus,
            self.inner.attention_budget,
            PyRecencyCurve {
                inner: self.inner.recency_curve.clone()
            }
            .__repr__()
        )
    }
}

/// Parse a string to ExperienceTypeTag (case-insensitive).
fn parse_experience_type_tag(s: &str) -> Option<ExperienceTypeTag> {
    match s.to_lowercase().as_str() {
        "difficulty" => Some(ExperienceTypeTag::Difficulty),
        "solution" => Some(ExperienceTypeTag::Solution),
        "errorpattern" | "error_pattern" => Some(ExperienceTypeTag::ErrorPattern),
        "successpattern" | "success_pattern" => Some(ExperienceTypeTag::SuccessPattern),
        "userpreference" | "user_preference" => Some(ExperienceTypeTag::UserPreference),
        "architecturaldecision" | "architectural_decision" => {
            Some(ExperienceTypeTag::ArchitecturalDecision)
        }
        "techinsight" | "tech_insight" => Some(ExperienceTypeTag::TechInsight),
        "fact" => Some(ExperienceTypeTag::Fact),
        "generic" => Some(ExperienceTypeTag::Generic),
        _ => None,
    }
}

/// Register type classes with the Python module.
pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyLlmConfig>()?;
    m.add_class::<PyLens>()?;
    m.add_class::<PyRecencyCurve>()?;
    Ok(())
}
