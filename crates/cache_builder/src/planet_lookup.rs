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
    pub fn prepare_blob(&mut self, block: &osmpbf::PrimitiveBlock) -> Result<(), String> {
        if let Self::Compact(nodes) = self {
            nodes.prefetch(
                block
                    .elements()
                    .filter_map(|e| {
                        if let osmpbf::Element::Way(way) = e {
                            Some(way)
                        } else {
                            None
                        }
                    })
                    .flat_map(|way| way.refs()),
            )?;
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
