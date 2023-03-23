use std::collections::{HashMap, HashSet};

use crate::analyzer::{Expression, Identity, IdentityKind, SelectedExpressions};
use crate::commit_evaluator::eval_error;
use crate::commit_evaluator::expression_evaluator::ExpressionEvaluator;
use crate::commit_evaluator::machine::LookupReturn;
use crate::commit_evaluator::util::contains_witness_ref;
use crate::number::{AbstractNumberType, DegreeType};

use super::affine_expression::AffineExpression;
use super::eval_error::EvalError;
use super::expression_evaluator::SymbolicVariables;
use super::machine::{LookupResult, Machine};
use super::{EvalResult, FixedData};

/// Machine to perform a lookup in fixed columns only.
/// It only supports lookup in the first column of the query and will use the first match.
pub struct FixedLookup {}

impl FixedLookup {
    pub fn try_new(
        _fixed_data: &FixedData,
        identities: &[&Identity],
        witness_names: &HashSet<&str>,
    ) -> Option<Box<Self>> {
        if identities.is_empty() && witness_names.is_empty() {
            Some(Box::new(FixedLookup {}))
        } else {
            None
        }
    }
}

impl Machine for FixedLookup {
    fn process_plookup(
        &mut self,
        fixed_data: &FixedData,
        kind: IdentityKind,
        left: &[Result<AffineExpression, EvalError>],
        right: &SelectedExpressions,
    ) -> LookupResult {
        // This is a matching machine if it is a plookup and the RHS is fully constant.
        if kind != IdentityKind::Plookup
            || right.selector.is_some()
            || right
                .expressions
                .iter()
                .any(|e| contains_witness_ref(e, fixed_data))
        {
            return Ok(LookupReturn::NotApplicable);
        }

        // If we already know the LHS, skip it.
        if left
            .iter()
            .all(|v| v.is_ok() && v.as_ref().unwrap().is_constant())
        {
            return Ok(LookupReturn::Assignments(vec![]));
        }

        let left_key = match left[0].clone() {
            Ok(v) => match v.constant_value() {
                Some(v) => Ok(v),
                None => Err(format!(
                    "First expression needs to be constant but is not: {}.",
                    v.format(fixed_data)
                )),
            },
            Err(err) => Err(format!("First expression on the LHS is unknown: {err}")),
        }?;

        let right_key = right.expressions.first().unwrap();
        let rhs_row = if let Expression::PolynomialReference(poly) = right_key {
            // TODO we really need a search index on this.
            fixed_data.fixed_cols
                .get(poly.name.as_str())
                .and_then(|values| values.iter().position(|v| *v == left_key))
                .ok_or_else(|| {
                    format!(
                        "Unable to find matching row on the RHS where the first element is {left_key} - only fixed columns supported there."
                    )
                })
                .map(|i| i as DegreeType)
        } else {
            Err("First item on the RHS must be a polynomial reference.".to_string())
        }?;

        // TODO we only support the following case:
        // - The first component on the LHS has to be known
        // - The first component on the RHS has to be a direct fixed column reference
        // - The first match of those uniquely determines the rest of the RHS.

        let mut reasons = vec![];
        let mut result = vec![];
        for (l, r) in left.iter().zip(right.expressions.iter()).skip(1) {
            match l {
                Ok(l) => match self.equate_to_constant_rhs(l, r, fixed_data, rhs_row) {
                    Ok(assignments) => result.extend(assignments),
                    Err(err) => reasons.push(err),
                },
                Err(err) => {
                    reasons.push(format!("Value of LHS component too complex: {err}").into());
                }
            }
        }
        if result.is_empty() {
            Err(reasons.into_iter().reduce(eval_error::combine).unwrap())
        } else {
            Ok(LookupReturn::Assignments(result))
        }
    }
    fn witness_col_values(
        &mut self,
        _fixed_data: &FixedData,
    ) -> HashMap<String, Vec<AbstractNumberType>> {
        Default::default()
    }
}

impl FixedLookup {
    fn equate_to_constant_rhs(
        &self,
        l: &AffineExpression,
        r: &Expression,
        fixed_data: &FixedData,
        rhs_row: DegreeType,
    ) -> EvalResult {
        let rhs_evaluator = ExpressionEvaluator::new(EvaluateFixedOnRow {
            fixed_data,
            row: rhs_row,
        });

        // This needs to be a costant because symbolic variables
        // would reference a different row!
        let r = rhs_evaluator.evaluate(r).and_then(|r| {
            r.constant_value()
                .ok_or_else(|| format!("Constant value required: {}", r.format(fixed_data)).into())
        })?;

        let evaluated = l.clone() - r.clone().into();
        match evaluated.solve() {
            Some((id, value)) => Ok(vec![(id, value)]),
            None => {
                let formatted = l.format(fixed_data);
                Err(if evaluated.is_invalid() {
                    format!("Constraint is invalid ({formatted} != {r}).",).into()
                } else {
                    format!("Could not solve expression {formatted} = {r}.",).into()
                })
            }
        }
    }
}

/// Evaluates references to fixed columns on a specific row.
struct EvaluateFixedOnRow<'a> {
    pub fixed_data: &'a FixedData<'a>,
    pub row: DegreeType,
}

impl<'a> SymbolicVariables for EvaluateFixedOnRow<'a> {
    fn constant(&self, name: &str) -> Result<AffineExpression, EvalError> {
        Ok(self.fixed_data.constants[name].clone().into())
    }

    fn value(&self, name: &str, next: bool) -> Result<AffineExpression, EvalError> {
        // TODO arrays
        let values = self.fixed_data.fixed_cols[name];
        let degree = values.len() as DegreeType;
        let row = if next {
            (self.row + 1) % degree
        } else {
            self.row
        };
        Ok(values[row as usize].clone().into())
    }

    fn format(&self, expr: AffineExpression) -> String {
        expr.format(self.fixed_data)
    }
}
