use std::path::PathBuf;

pub mod errors;
use oxc_diagnostics::{Error, FailedToOpenFileError, Report};
use phf::{phf_map, Map};
use serde_json::Value;

use crate::{
    rules::{RuleEnum, RULES},
    AllowWarnDeny,
};

use self::errors::{
    FailedToParseConfigError, FailedToParseConfigJsonError, FailedToParseConfigPropertyError,
    FailedToParseRuleValueError,
};

pub struct ESLintConfig {
    rules: std::vec::Vec<RuleEnum>,
}

impl ESLintConfig {
    pub fn new(path: &PathBuf) -> Result<Self, Report> {
        let file = match std::fs::read_to_string(path) {
            Ok(file) => file,
            Err(e) => {
                return Err(FailedToParseConfigError(vec![Error::new(FailedToOpenFileError(
                    path.clone(),
                    e,
                ))])
                .into());
            }
        };

        let file = match serde_json::from_str::<serde_json::Value>(&file) {
            Ok(file) => file,
            Err(e) => {
                return Err(FailedToParseConfigError(vec![Error::new(
                    FailedToParseConfigJsonError(path.clone(), e.to_string()),
                )])
                .into());
            }
        };

        let extends_hm = match parse_extends(&file) {
            Ok(Some(extends_hm)) => {
                extends_hm.into_iter().collect::<std::collections::HashSet<_>>()
            }
            Ok(None) => std::collections::HashSet::new(),
            Err(e) => {
                return Err(FailedToParseConfigError(vec![Error::new(
                    FailedToParseConfigJsonError(path.clone(), e.to_string()),
                )])
                .into());
            }
        };
        let roles_hm = match parse_rules(&file) {
            Ok(roles_hm) => roles_hm
                .into_iter()
                .map(|(plugin_name, rule_name, allow_warn_deny, config)| {
                    ((plugin_name, rule_name), (allow_warn_deny, config))
                })
                .collect::<std::collections::HashMap<_, _>>(),
            Err(e) => {
                return Err(e);
            }
        };

        // `extends` provides the defaults
        // `rules` provides the overrides
        let rules = RULES.clone().into_iter().filter_map(|rule| {
            // Check if the extends set is empty or contains the plugin name
            let in_extends = extends_hm.contains(rule.plugin_name());

            // Check if there's a custom rule that explicitly handles this rule
            let (is_explicitly_handled, policy, config) =
                if let Some((policy, config)) = roles_hm.get(&(rule.plugin_name(), rule.name())) {
                    // Return true for handling, and also whether it's enabled or not
                    (true, *policy, config)
                } else {
                    // Not explicitly handled
                    (false, AllowWarnDeny::Allow, &None)
                };

            // The rule is included if it's in the extends set and not explicitly disabled,
            // or if it's explicitly enabled
            if (in_extends && !is_explicitly_handled) || policy.is_enabled() {
                Some(rule.read_json(config.cloned()))
            } else {
                None
            }
        });

        Ok(Self { rules: rules.collect::<Vec<_>>() })
    }

    pub fn into_rules(mut self) -> Vec<RuleEnum> {
        self.rules.sort_unstable_by_key(RuleEnum::name);
        self.rules
    }
}

fn parse_extends(root_json: &Value) -> Result<Option<Vec<&'static str>>, Report> {
    let Some(extends) = root_json.get("extends") else {
        return Ok(None);
    };

    let extends_obj = match extends {
        Value::Array(v) => v,
        _ => {
            return Err(FailedToParseConfigPropertyError("extends", "Expected an array.").into());
        }
    };

    let extends_rule_groups = extends_obj
        .iter()
        .filter_map(|v| {
            let v = match v {
                Value::String(s) => s,
                _ => return None,
            };

            if let Some(m) = EXTENDS_MAP.get(v.as_str()) {
                return Some(*m);
            }

            None
        })
        .collect::<Vec<_>>();

    Ok(Some(extends_rule_groups))
}

#[allow(clippy::type_complexity)]
fn parse_rules(
    root_json: &Value,
) -> Result<Vec<(&str, &str, AllowWarnDeny, Option<&Value>)>, Error> {
    let Value::Object(rules_object) = root_json else { return Ok(vec![]) };

    let Some(Value::Object(rules_object)) = rules_object.get("rules") else { return Ok(vec![]) };

    rules_object
        .iter()
        .map(|(key, value)| {
            let (plugin_name, name) = parse_rule_name(key);

            let (rule_severity, rule_config) = resolve_rule_value(value)?;

            Ok((plugin_name, name, rule_severity, rule_config))
        })
        .collect::<Result<Vec<_>, Error>>()
}

pub const EXTENDS_MAP: Map<&'static str, &'static str> = phf_map! {
    "eslint:recommended" => "eslint",
    "plugin:react/recommended" => "react",
    "plugin:@typescript-eslint/recommended" => "typescript",
    "plugin:react-hooks/recommended" => "react",
    "plugin:unicorn/recommended" => "unicorn",
    "plugin:jest/recommended" => "jest",
};

fn parse_rule_name(name: &str) -> (&str, &str) {
    if let Some((category, name)) = name.split_once('/') {
        let category = category.trim_start_matches('@');

        // if it matches typescript-eslint, map it to typescript
        let category = match category {
            "typescript-eslint" => "typescript",
            _ => category,
        };

        (category, name)
    } else {
        ("eslint", name)
    }
}

/// Resolves the level of a rule and its config
///
/// Two cases here
/// ```json
/// {
///     "rule": "off",
///     "rule": ["off", "config"],
/// }
/// ```
fn resolve_rule_value(value: &serde_json::Value) -> Result<(AllowWarnDeny, Option<&Value>), Error> {
    if let Some(v) = value.as_str() {
        return Ok((AllowWarnDeny::try_from(v)?, None));
    }

    if let Some(v) = value.as_array() {
        if let Some(v_idx_0) = v.get(0) {
            return Ok((AllowWarnDeny::try_from(v_idx_0)?, v.get(1)));
        }
    }

    Err(FailedToParseRuleValueError(value.to_string(), "Invalid rule value").into())
}
