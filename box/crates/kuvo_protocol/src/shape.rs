//! The route shape language, shared by the build generator and the JSON checker.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Shape {
    Primitive(Primitive),
    Compound(Compound),
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Primitive {
    String,
    Number,
    Bool,
    Null,
    Any,
    Bytes,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Compound {
    Literal(Literal),
    List(Child),
    Map(Child),
    Object(Fields),
    OneOf(Cow<'static, [Child]>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Child {
    Static(&'static Shape),
    Owned(Box<Shape>),
}

impl Child {
    pub fn shape(&self) -> &Shape {
        match self {
            Self::Static(shape) => shape,
            Self::Owned(shape) => shape,
        }
    }
}

impl<'de> Deserialize<'de> for Child {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::deserialize(deserializer).map(Self::Owned)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: Cow<'static, str>,
    pub optional: bool,
    pub shape: Child,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fields(pub Cow<'static, [Field]>);

impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = BTreeMap::<String, Shape>::deserialize(deserializer)?;
        let fields = entries
            .into_iter()
            .map(|(mut name, shape)| {
                let optional = name.ends_with('?');
                if optional {
                    name.pop();
                }
                Field {
                    name: name.into(),
                    optional,
                    shape: Child::Owned(Box::new(shape)),
                }
            })
            .collect::<Vec<_>>();
        Ok(Self(fields.into()))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Literal {
    String(Cow<'static, str>),
    Number(f64),
    Bool(bool),
    Null,
}
