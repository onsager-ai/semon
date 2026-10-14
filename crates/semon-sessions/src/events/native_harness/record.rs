//! The native record producer reads properties through either Value or the
//! qualified Original descriptor owner. Property qualification can defer; the
//! semantic cursor advances only after the selected action succeeds.

use serde_json::Value;
use std::convert::Infallible;

pub(crate) trait RecordSource {
    type Error;
    type Node: Clone;
    type Text: Clone;
    fn root(&self) -> Self::Node;
    fn member(
        &mut self,
        node: &Self::Node,
        name: &'static str,
    ) -> Result<Option<Self::Node>, Self::Error>;
    fn text(&mut self, node: &Self::Node) -> Result<Option<Self::Text>, Self::Error>;
    fn string_present(&mut self, node: &Self::Node) -> Result<bool, Self::Error>;
    fn string_nonempty(&mut self, node: &Self::Node) -> Result<bool, Self::Error>;
    fn text_nonempty(&mut self, text: &Self::Text) -> Result<bool, Self::Error>;
    fn text_literal(
        &mut self,
        text: &Self::Text,
        literal: &'static str,
    ) -> Result<bool, Self::Error>;
    fn literal(&mut self, node: &Self::Node, literal: &'static str) -> Result<bool, Self::Error>;
    fn boolean(&mut self, node: &Self::Node) -> Result<Option<bool>, Self::Error>;
    fn integer(&mut self, node: &Self::Node) -> Result<Option<i64>, Self::Error>;
    fn array_next(
        &mut self,
        node: &Self::Node,
        after: Option<u64>,
    ) -> Result<Option<(u64, Self::Node)>, Self::Error>;
}

pub(crate) struct ValueRecord<'a> {
    pub(crate) record: &'a Value,
}
impl<'a> RecordSource for ValueRecord<'a> {
    type Error = Infallible;
    type Node = &'a Value;
    type Text = String;
    fn root(&self) -> Self::Node {
        self.record
    }
    fn member(
        &mut self,
        node: &Self::Node,
        name: &'static str,
    ) -> Result<Option<Self::Node>, Infallible> {
        Ok(node.get(name))
    }
    fn text(&mut self, node: &Self::Node) -> Result<Option<String>, Infallible> {
        Ok(node.as_str().map(str::to_owned))
    }
    fn string_present(&mut self, node: &Self::Node) -> Result<bool, Infallible> {
        Ok(node.is_string())
    }
    fn string_nonempty(&mut self, node: &Self::Node) -> Result<bool, Infallible> {
        Ok(node.as_str().is_some_and(|s| !s.is_empty()))
    }
    fn text_nonempty(&mut self, text: &String) -> Result<bool, Infallible> {
        Ok(!text.is_empty())
    }
    fn text_literal(&mut self, text: &String, literal: &'static str) -> Result<bool, Infallible> {
        Ok(text == literal)
    }
    fn literal(&mut self, node: &Self::Node, literal: &'static str) -> Result<bool, Infallible> {
        Ok(node.as_str() == Some(literal))
    }
    fn boolean(&mut self, node: &Self::Node) -> Result<Option<bool>, Infallible> {
        Ok(node.as_bool())
    }
    fn integer(&mut self, node: &Self::Node) -> Result<Option<i64>, Infallible> {
        Ok(node.as_i64())
    }
    fn array_next(
        &mut self,
        node: &Self::Node,
        after: Option<u64>,
    ) -> Result<Option<(u64, Self::Node)>, Infallible> {
        let Some(array) = node.as_array() else {
            return Ok(None);
        };
        let index = after.map_or(Some(0), |index| index.checked_add(1));
        Ok(index.and_then(|index| {
            usize::try_from(index)
                .ok()
                .and_then(|at| array.get(at).map(|node| (index, node)))
        }))
    }
}

pub(crate) fn member_text<R: RecordSource>(
    record: &mut R,
    parent: Option<&R::Node>,
    name: &'static str,
) -> Result<Option<R::Text>, R::Error> {
    match parent {
        Some(parent) => match record.member(parent, name)? {
            Some(node) => record.text(&node),
            None => Ok(None),
        },
        None => Ok(None),
    }
}
pub(crate) fn member_bool<R: RecordSource>(
    record: &mut R,
    parent: Option<&R::Node>,
    name: &'static str,
) -> Result<Option<bool>, R::Error> {
    match parent {
        Some(parent) => match record.member(parent, name)? {
            Some(node) => record.boolean(&node),
            None => Ok(None),
        },
        None => Ok(None),
    }
}
pub(crate) fn member_literal<R: RecordSource>(
    record: &mut R,
    parent: Option<&R::Node>,
    name: &'static str,
    literal: &'static str,
) -> Result<bool, R::Error> {
    match parent {
        Some(parent) => match record.member(parent, name)? {
            Some(node) => record.literal(&node, literal),
            None => Ok(false),
        },
        None => Ok(false),
    }
}
