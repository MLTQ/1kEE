use crate::{flat_node_store, pbf_nodes};

pub enum Lookup {
    Flat(flat_node_store::NodeLookup),
    Compact(pbf_nodes::PbfNodes),
}
pub enum Session<'a> {
    Flat(flat_node_store::LookupSession<'a>),
    Compact(pbf_nodes::Session<'a>),
}
impl Lookup {
    pub fn is_compact(&self) -> bool {
        matches!(self, Self::Compact(_))
    }
    pub fn prepare_batch(
        &self,
        ids: Vec<i64>,
    ) -> Result<Option<std::sync::Arc<pbf_nodes::Prepared>>, String> {
        match self {
            Self::Flat(_) => Ok(None),
            Self::Compact(nodes) => nodes.prepare_batch(ids).map(Some),
        }
    }
    pub fn prepared_session(
        &self,
        prepared: &Option<std::sync::Arc<pbf_nodes::Prepared>>,
    ) -> Session<'_> {
        match (self, prepared) {
            (Self::Compact(nodes), Some(prepared)) => {
                Session::Compact(nodes.prepared_session(prepared.clone()))
            }
            _ => self.session(),
        }
    }

    pub fn activity(&self) -> (u64, u64, u64) {
        match self {
            Self::Flat(_) => (0, 0, 0),
            Self::Compact(n) => {
                let (blocks, bytes) = n.activity();
                (blocks, bytes, n.node_count)
            }
        }
    }

    pub fn session(&self) -> Session<'_> {
        match self {
            Self::Flat(n) => Session::Flat(n.session()),
            Self::Compact(n) => Session::Compact(n.session()),
        }
    }
    pub fn validate_source(&self) -> Result<(), String> {
        match self {
            Self::Flat(_) => Ok(()),
            Self::Compact(n) => n.validate_source(),
        }
    }
}
impl Session<'_> {
    pub fn prepare_refs(&mut self, refs: impl Iterator<Item = i64>) -> Result<(), String> {
        if let Self::Compact(nodes) = self {
            nodes.prefetch(refs)?;
        }
        Ok(())
    }

    pub fn lookup_many(&mut self, ids: &[i64]) -> Result<Vec<Option<(f32, f32)>>, String> {
        match self {
            Self::Flat(n) => n.lookup_many(ids),
            Self::Compact(n) => n.lookup_many(ids),
        }
    }
}
