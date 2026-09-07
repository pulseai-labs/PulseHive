//! Config type bindings: LlmConfig, RecencyCurve, Lens.

use std::collections::HashMap;

use pulsehive_core::lens::{ExperienceTypeTag, Lens, RecencyCurve};
use pulsehive_core::llm::LlmConfig;

#[cfg(feature = "napi")]
use napi_derive::napi;

// ── LlmConfig ────────────────────────────────────────────────────────

/// LLM model selection and generation parameters.
#[cfg_attr(feature = "napi", napi)]
pub struct JsLlmConfig {
    pub(crate) inner: LlmConfig,
}

#[cfg_attr(feature = "napi", napi)]
impl JsLlmConfig {
    /// Create a new LlmConfig.
    ///
    /// @param provider - Provider name matching HiveMind builder key (e.g., "openai", "anthropic")
    /// @param model - Model identifier (e.g., "gpt-4", "claude-sonnet-4-6")
    /// @param temperature - Sampling temperature (0.0 = deterministic, 1.0+ = creative). Default: 0.7
    /// @param maxTokens - Maximum tokens to generate. Default: 4096
    #[cfg_attr(feature = "napi", napi(constructor))]
    pub fn new(
        provider: String,
        model: String,
        temperature: Option<f64>,
        max_tokens: Option<u32>,
    ) -> Self {
        Self {
            inner: LlmConfig::new(provider, model)
                .with_temperature(temperature.unwrap_or(0.7) as f32)
                .with_max_tokens(max_tokens.unwrap_or(4096)),
        }
    }

    /// Provider name.
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn provider(&self) -> String {
        self.inner.provider.clone()
    }

    /// Model identifier.
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn model(&self) -> String {
        self.inner.model.clone()
    }

    /// Sampling temperature.
    #[cfg_attr(feature = "napi", napi(getter))]
    pub fn temperature(&self) -> f64 {
        self.inner.temperature as f64
    }

    /// Maximum tokens to generate.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "maxTokens"))]
    pub fn max_tokens(&self) -> u32 {
        self.inner.max_tokens
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        format!(
            "LlmConfig(provider='{}', model='{}', temperature={}, maxTokens={})",
            self.inner.provider, self.inner.model, self.inner.temperature, self.inner.max_tokens
        )
    }
}

// ── RecencyCurve ─────────────────────────────────────────────────────

/// Time decay function controlling how recency affects perception.
///
/// Use static factory methods to create:
/// - `RecencyCurve.exponential(72.0)` — 72-hour half-life
/// - `RecencyCurve.uniform()` — no decay
#[cfg_attr(feature = "napi", napi)]
pub struct JsRecencyCurve {
    pub(crate) inner: RecencyCurve,
}

#[cfg_attr(feature = "napi", napi)]
impl JsRecencyCurve {
    /// Create exponential decay with the given half-life in hours.
    ///
    /// Formula: weight = 0.5^(age_hours / half_life_hours)
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn exponential(half_life_hours: f64) -> Self {
        Self {
            inner: RecencyCurve::Exponential {
                half_life_hours: half_life_hours as f32,
            },
        }
    }

    /// Create uniform weighting (no temporal decay).
    #[cfg_attr(feature = "napi", napi(factory))]
    pub fn uniform() -> Self {
        Self {
            inner: RecencyCurve::Uniform,
        }
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
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
#[cfg_attr(feature = "napi", napi)]
pub struct JsLens {
    pub(crate) inner: Lens,
}

#[cfg_attr(feature = "napi", napi)]
impl JsLens {
    /// Create a new Lens.
    ///
    /// @param domains - List of domain focus strings (e.g., ["safety", "clinical"])
    /// @param attentionBudget - Max experiences to perceive per cycle. Default: 50
    /// @param recencyCurve - Temporal decay function. Default: exponential(72.0)
    /// @param typeWeights - Record mapping type names to weights. Default: all 1.0
    #[cfg_attr(feature = "napi", napi(constructor))]
    pub fn new(
        domains: Vec<String>,
        attention_budget: Option<u32>,
        recency_curve: Option<&JsRecencyCurve>,
        type_weights: Option<HashMap<String, f64>>,
    ) -> Self {
        let mut lens = Lens::new(domains);
        if let Some(budget) = attention_budget {
            lens.attention_budget = budget as usize;
        }
        if let Some(rc) = recency_curve {
            lens.recency_curve = rc.inner.clone();
        }
        if let Some(weights) = type_weights {
            for (key, value) in weights {
                if let Some(tag) = parse_experience_type_tag(&key) {
                    lens.type_weights.insert(tag, value as f32);
                }
            }
        }
        Self { inner: lens }
    }

    /// Domain focus strings.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "domainFocus"))]
    pub fn domain_focus(&self) -> Vec<String> {
        self.inner.domain_focus.clone()
    }

    /// Maximum experiences to perceive per cycle.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "attentionBudget"))]
    pub fn attention_budget(&self) -> u32 {
        self.inner.attention_budget as u32
    }

    /// Recency curve configuration.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "recencyCurve"))]
    pub fn recency_curve(&self) -> JsRecencyCurve {
        JsRecencyCurve {
            inner: self.inner.recency_curve.clone(),
        }
    }

    /// Type weights as a Record<string, number>.
    #[cfg_attr(feature = "napi", napi(getter, js_name = "typeWeights"))]
    pub fn type_weights(&self) -> HashMap<String, f64> {
        self.inner
            .type_weights
            .iter()
            .map(|(k, v)| (format!("{k:?}").to_lowercase(), *v as f64))
            .collect()
    }

    /// String representation for debugging.
    #[cfg_attr(feature = "napi", napi(js_name = "toString"))]
    pub fn to_string_js(&self) -> String {
        format!(
            "Lens(domains={:?}, attentionBudget={}, recencyCurve={})",
            self.inner.domain_focus,
            self.inner.attention_budget,
            match &self.inner.recency_curve {
                RecencyCurve::Exponential { half_life_hours } =>
                    format!("RecencyCurve.exponential({half_life_hours})"),
                RecencyCurve::Uniform => "RecencyCurve.uniform()".to_string(),
            }
        )
    }
}

/// Parse a string to ExperienceTypeTag (case-insensitive).
fn parse_experience_type_tag(s: &str) -> Option<ExperienceTypeTag> {
    match s.to_lowercase().as_str() {
        "difficulty" => Some(ExperienceTypeTag::Difficulty),
        "solution" => Some(ExperienceTypeTag::Solution),
        "errorpattern" | "error_pattern" | "errorPattern" => Some(ExperienceTypeTag::ErrorPattern),
        "successpattern" | "success_pattern" | "successPattern" => {
            Some(ExperienceTypeTag::SuccessPattern)
        }
        "userpreference" | "user_preference" | "userPreference" => {
            Some(ExperienceTypeTag::UserPreference)
        }
        "architecturaldecision" | "architectural_decision" | "architecturalDecision" => {
            Some(ExperienceTypeTag::ArchitecturalDecision)
        }
        "techinsight" | "tech_insight" | "techInsight" => Some(ExperienceTypeTag::TechInsight),
        "fact" => Some(ExperienceTypeTag::Fact),
        "generic" => Some(ExperienceTypeTag::Generic),
        _ => None,
    }
}
