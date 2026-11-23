use std::fmt;

use serde_json::Value;

use crate::routes::{PASSED_ON, REFUSAL_SHAPE, ROUTES};
use crate::shape::{Compound, Literal, Primitive, Shape};

/// One exported route pattern. A segment beginning with `:` accepts one nonempty path segment.
#[derive(Debug)]
pub struct Route {
    pub method: &'static str,
    pub path: &'static str,
    pub feature: &'static str,
    pub answers: &'static [(u16, &'static Shape)],
    pub body: Option<&'static Shape>,
}

impl Route {
    pub fn answer(&self, status: u16) -> Option<&'static Shape> {
        self.answers
            .iter()
            .find(|(code, _)| *code == status)
            .map(|(_, shape)| *shape)
    }
}

fn matches_path(pattern: &str, path: &str) -> bool {
    let mut actual = path.split('/');
    for part in pattern.split('/') {
        let Some(candidate) = actual.next() else {
            return false;
        };
        if part != candidate && !(part.starts_with(':') && !candidate.is_empty()) {
            return false;
        }
    }
    actual.next().is_none()
}

fn path_without_query(path: &str) -> &str {
    path.split_once('?').map_or(path, |(path, _)| path)
}

/// Match a request as the contract runner does, including the GET fallback for HEAD.
pub fn route(method: &str, path: &str) -> Option<&'static Route> {
    let path = path_without_query(path);
    let find = |method: &str| {
        ROUTES.iter().find(|route| {
            route.method.eq_ignore_ascii_case(method) && matches_path(route.path, path)
        })
    };
    find(method).or_else(|| {
        if method.eq_ignore_ascii_case("HEAD") {
            find("GET")
        } else {
            None
        }
    })
}

#[derive(Debug)]
pub enum AnswerError {
    UnknownRoute {
        method: String,
        path: String,
    },
    UnexpectedStatus {
        route: &'static Route,
        status: u16,
    },
    ExpectedJson {
        route: &'static Route,
        content_type: String,
    },
    InvalidJson(serde_json::Error),
    Shape(ShapeError),
}

impl fmt::Display for AnswerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRoute { method, path } => {
                write!(formatter, "{method} {path} has no exported shape")
            }
            Self::UnexpectedStatus { route, status } => write!(
                formatter,
                "{} {} has no shape for {status}",
                route.method, route.path
            ),
            Self::ExpectedJson {
                route,
                content_type,
            } => write!(
                formatter,
                "{} {} expected a JSON answer, got {content_type:?}",
                route.method, route.path
            ),
            Self::InvalidJson(error) => write!(formatter, "invalid JSON answer: {error}"),
            Self::Shape(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for AnswerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidJson(error) => Some(error),
            Self::Shape(error) => Some(error),
            _ => None,
        }
    }
}

/// Validate a route's raw answer using the exported contract.
pub fn check_answer(
    method: &str,
    path: &str,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), AnswerError> {
    let path = path_without_query(path);
    let known = route(method, path);
    if known.is_none() && PASSED_ON.iter().any(|prefix| path.starts_with(prefix)) {
        return Ok(());
    }
    let is_json = content_type.starts_with("application/json");
    let shape = if status >= 400 {
        if !is_json {
            return Ok(());
        }
        known
            .and_then(|route| route.answer(status))
            .unwrap_or(REFUSAL_SHAPE)
    } else {
        let route = known.ok_or_else(|| AnswerError::UnknownRoute {
            method: method.to_owned(),
            path: path.to_owned(),
        })?;
        if method.eq_ignore_ascii_case("HEAD") && body.is_empty() {
            return Ok(());
        }
        let shape = route
            .answer(status)
            .ok_or(AnswerError::UnexpectedStatus { route, status })?;
        if matches!(shape, Shape::Primitive(Primitive::Bytes)) {
            return Ok(());
        }
        if !is_json {
            return Err(AnswerError::ExpectedJson {
                route,
                content_type: content_type.to_owned(),
            });
        }
        shape
    };
    let value = serde_json::from_slice(body).map_err(AnswerError::InvalidJson)?;
    check_shape(shape, Some(&value)).map_err(AnswerError::Shape)
}

#[derive(Debug)]
pub struct ShapeError {
    path: Vec<String>,
    reason: String,
}

impl ShapeError {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            path: Vec::new(),
            reason: reason.into(),
        }
    }

    fn at(mut self, part: String) -> Self {
        self.path.push(part);
        self
    }
}

impl fmt::Display for ShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("body")?;
        for part in self.path.iter().rev() {
            formatter.write_str(part)?;
        }
        write!(formatter, " {}", self.reason)
    }
}

impl std::error::Error for ShapeError {}

/// A missing value differs from a present JSON null, even for an open shape.
pub fn check_shape(shape: &Shape, value: Option<&Value>) -> Result<(), ShapeError> {
    let value = value.ok_or_else(|| ShapeError::new("is missing"))?;
    match shape {
        Shape::Primitive(kind) => {
            let matches = match kind {
                Primitive::String => value.is_string(),
                Primitive::Number => value.is_number(),
                Primitive::Bool => value.is_boolean(),
                Primitive::Null => value.is_null(),
                Primitive::Any => true,
                Primitive::Bytes => false,
            };
            matches
                .then_some(())
                .ok_or_else(|| ShapeError::new(format!("does not match {kind:?}")))
        }
        Shape::Compound(compound) => match compound {
            Compound::Literal(literal) => {
                let matches = match literal {
                    Literal::String(expected) => value.as_str() == Some(expected),
                    Literal::Number(expected) => value.as_f64() == Some(*expected),
                    Literal::Bool(expected) => value.as_bool() == Some(*expected),
                    Literal::Null => value.is_null(),
                };
                matches
                    .then_some(())
                    .ok_or_else(|| ShapeError::new(format!("does not match {literal:?}")))
            }
            Compound::List(child) => {
                let items = value
                    .as_array()
                    .ok_or_else(|| ShapeError::new("must be an array"))?;
                for (index, item) in items.iter().enumerate() {
                    check_shape(child.shape(), Some(item))
                        .map_err(|error| error.at(format!("[{index}]")))?;
                }
                Ok(())
            }
            Compound::Map(child) => {
                let object = value
                    .as_object()
                    .ok_or_else(|| ShapeError::new("must be an object"))?;
                for (name, item) in object {
                    check_shape(child.shape(), Some(item))
                        .map_err(|error| error.at(format!(".{name}")))?;
                }
                Ok(())
            }
            Compound::Object(fields) => {
                let object = value
                    .as_object()
                    .ok_or_else(|| ShapeError::new("must be an object"))?;
                for name in object.keys() {
                    if !fields.0.iter().any(|field| field.name == name.as_str()) {
                        return Err(
                            ShapeError::new("is an unexpected field").at(format!(".{name}"))
                        );
                    }
                }
                for field in fields.0.iter() {
                    let item = object.get(field.name.as_ref());
                    if item.is_some() || !field.optional {
                        check_shape(field.shape.shape(), item)
                            .map_err(|error| error.at(format!(".{}", field.name)))?;
                    }
                }
                Ok(())
            }
            Compound::OneOf(choices) => {
                let mut failures = Vec::new();
                for choice in choices.iter() {
                    match check_shape(choice.shape(), Some(value)) {
                        Ok(()) => return Ok(()),
                        Err(error) => failures.push(error),
                    }
                }
                // Each alternative already has an error at its first failing field.
                let reasons: Vec<_> = failures.iter().map(ToString::to_string).collect();
                Err(ShapeError::new(format!(
                    "matches none of its alternatives ({})",
                    reasons.join("; ")
                )))
            }
        },
    }
}
