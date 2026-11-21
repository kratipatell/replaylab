use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::optimizer::{self, OptimizeSpec, ParamKind, ParamSpec, SearchMethod, SearchSpec};
use crate::strategy::Strategy;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedGridParameter {
    pub target: String,
    pub kind: ParamKind,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyTemplate {
    pub id: String,
    pub name: String,
    pub description: String,
    pub market_idea: String,
    pub typical_failure_mode: String,
    pub recommended_timeframe: String,
    pub suggested_walk_forward_parameter_grid: Vec<SuggestedGridParameter>,
    pub strategy: Strategy,
}

#[derive(Debug, thiserror::Error)]
pub enum TemplateCatalogError {
    #[error("could not parse strategy templates: {0}")]
    Json(#[from] serde_json::Error),
    #[error("strategy template catalog is empty")]
    Empty,
    #[error("template ID {0:?} is duplicated")]
    DuplicateId(String),
    #[error("template {id:?} has an empty {field}")]
    EmptyMetadata { id: String, field: &'static str },
    #[error("template ID {0:?} must use lowercase letters, digits, and hyphens only")]
    InvalidId(String),
    #[error("template {id:?} recommended timeframe differs from its strategy timeframe")]
    TimeframeMismatch { id: String },
    #[error("template {id:?} has an invalid strategy: {message}")]
    InvalidStrategy { id: String, message: String },
    #[error("template {id:?} has an invalid walk-forward grid: {message}")]
    InvalidGrid { id: String, message: String },
}

#[derive(Debug, Clone)]
pub struct TemplateCatalog {
    templates: Vec<StrategyTemplate>,
}

impl TemplateCatalog {
    pub fn from_json(text: &str) -> Result<Self, TemplateCatalogError> {
        let templates: Vec<StrategyTemplate> = serde_json::from_str(text)?;
        if templates.is_empty() {
            return Err(TemplateCatalogError::Empty);
        }

        let mut ids = HashSet::with_capacity(templates.len());
        for template in &templates {
            validate_metadata(template)?;
            if !ids.insert(template.id.as_str()) {
                return Err(TemplateCatalogError::DuplicateId(template.id.clone()));
            }
            crate::agent::validate_new(
                &template.name,
                &template.strategy,
                crate::agent::DEFAULT_INITIAL_EQUITY,
            )
            .map_err(|error| TemplateCatalogError::InvalidStrategy {
                id: template.id.clone(),
                message: error.to_string(),
            })?;
            validate_grid(template)?;
        }
        Ok(Self { templates })
    }

    pub fn all(&self) -> &[StrategyTemplate] {
        &self.templates
    }

    pub fn get(&self, id: &str) -> Option<&StrategyTemplate> {
        self.templates.iter().find(|template| template.id == id)
    }
}

fn validate_metadata(template: &StrategyTemplate) -> Result<(), TemplateCatalogError> {
    if template.id.trim().is_empty()
        || !template
            .id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(TemplateCatalogError::InvalidId(template.id.clone()));
    }
    for (field, value) in [
        ("name", template.name.as_str()),
        ("description", template.description.as_str()),
        ("market_idea", template.market_idea.as_str()),
        (
            "typical_failure_mode",
            template.typical_failure_mode.as_str(),
        ),
        (
            "recommended_timeframe",
            template.recommended_timeframe.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(TemplateCatalogError::EmptyMetadata {
                id: template.id.clone(),
                field,
            });
        }
    }
    if template.strategy.timeframe != template.recommended_timeframe {
        return Err(TemplateCatalogError::TimeframeMismatch {
            id: template.id.clone(),
        });
    }
    Ok(())
}

fn validate_grid(template: &StrategyTemplate) -> Result<(), TemplateCatalogError> {
    let params = template
        .suggested_walk_forward_parameter_grid
        .iter()
        .map(|grid| ParamSpec {
            target: grid.target.clone(),
            kind: Some(grid.kind),
            values: Some(grid.values.clone()),
            min: None,
            max: None,
            step: None,
        })
        .collect();
    optimizer::build_combos(&OptimizeSpec {
        strategy: template.strategy.clone(),
        start_ms: 0,
        end_ms: 1,
        fee_bps: None,
        slippage_bps: None,
        params,
        search: SearchSpec {
            method: SearchMethod::Grid,
            trials: None,
            seed: None,
        },
        validation: None,
        objective: None,
    })
    .map(|_| ())
    .map_err(|error| TemplateCatalogError::InvalidGrid {
        id: template.id.clone(),
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::BacktestConfig;
    use crate::types::Candle;

    fn fixture() -> Vec<Candle> {
        let mut previous = 100.0;
        (0..320)
            .map(|index| {
                let close =
                    100.0 + (index as f64 * 0.17).sin() * 7.0 + (index as f64 * 0.031).cos() * 2.0;
                let open = previous;
                previous = close;
                Candle {
                    ts: 1_700_000_000_000 + index as i64 * 3_600_000,
                    open,
                    high: open.max(close) + 0.8,
                    low: open.min(close) - 0.8,
                    close,
                }
            })
            .collect()
    }

    #[test]
    fn all_templates_validate_and_backtest_deterministically() {
        let catalog = TemplateCatalog::from_json(include_str!("../templates.json"))
            .expect("embedded templates");
        let candles = fixture();
        let cfg = BacktestConfig::default();
        let ids: HashSet<&str> = catalog
            .all()
            .iter()
            .map(|template| template.id.as_str())
            .collect();
        assert_eq!(ids.len(), catalog.all().len());
        assert_eq!(catalog.all().len(), 6);

        for template in catalog.all() {
            crate::agent::validate_new(
                &template.name,
                &template.strategy,
                crate::agent::DEFAULT_INITIAL_EQUITY,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", template.id));
            let first = crate::backtest::backtest_strategy(&template.strategy, &candles, &cfg)
                .unwrap_or_else(|error| panic!("{}: {error}", template.id));
            let second = crate::backtest::backtest_strategy(&template.strategy, &candles, &cfg)
                .unwrap_or_else(|error| panic!("{}: {error}", template.id));
            assert_eq!(first, second, "template {}", template.id);
            assert_eq!(first.equity_curve.len(), candles.len(), "{}", template.id);
        }
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../templates.json")).expect("template JSON");
        let templates = value.as_array_mut().expect("array");
        templates[1]["id"] = templates[0]["id"].clone();

        assert!(matches!(
            TemplateCatalog::from_json(&value.to_string()),
            Err(TemplateCatalogError::DuplicateId(_))
        ));
    }

    #[test]
    fn invalid_template_fails_catalog_loading() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../templates.json")).expect("template JSON");
        value[0]["strategy"]["entry"] = serde_json::json!({"all": [], "any": []});

        assert!(matches!(
            TemplateCatalog::from_json(&value.to_string()),
            Err(TemplateCatalogError::InvalidStrategy { .. })
        ));
    }

    #[test]
    fn old_or_invalid_metadata_is_not_silently_skipped() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../templates.json")).expect("template JSON");
        value[0]["suggested_walk_forward_parameter_grid"][0]["target"] =
            serde_json::json!("entry.all.99.right");

        assert!(matches!(
            TemplateCatalog::from_json(&value.to_string()),
            Err(TemplateCatalogError::InvalidGrid { .. })
        ));
    }
}
