//! Parallel-decodable node batches; byte layout matches the flat node store.
use osmpbf::{Blob, BlobDecode, Element};

pub(super) fn node_records(blob: &Blob) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    if let BlobDecode::OsmData(block) = blob.decode().map_err(|e| e.to_string())? {
        for element in block.elements() {
            let (id, lat, lon) = match element {
                Element::Node(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                Element::DenseNode(n) => (n.id(), n.lat() as f32, n.lon() as f32),
                _ => continue,
            };
            bytes.extend_from_slice(&id.to_le_bytes());
            bytes.extend_from_slice(&lat.to_le_bytes());
            bytes.extend_from_slice(&lon.to_le_bytes());
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    #[test]
    fn parallel_plain_and_dense_batches_preserve_exact_record_order() {
        for bytes in [
            include_bytes!("../tests/fixtures/planet-tiny-dense.osm.pbf").as_slice(),
            include_bytes!("../tests/fixtures/planet-tiny-plain.osm.pbf").as_slice(),
        ] {
            let blobs: Vec<_> = osmpbf::BlobReader::new(bytes).map(Result::unwrap).collect();
            let actual = blobs
                .par_iter()
                .map(node_records)
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .concat();
            let expected: Vec<u8> = [
                (10_i64, 36.5_f32, -120.5_f32),
                (20, 36.501, -120.499),
                (30, 36.502, -120.498),
                (40, 36.503, -120.497),
            ]
            .into_iter()
            .flat_map(|(id, lat, lon)| {
                [
                    id.to_le_bytes().as_slice(),
                    lat.to_le_bytes().as_slice(),
                    lon.to_le_bytes().as_slice(),
                ]
                .concat()
            })
            .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn malformed_data_blob_is_an_error_instead_of_empty_nodes() {
        // Valid PBF envelope with an invalid one-byte protobuf payload.
        let bytes = b"\0\0\0\x0b\x0a\x07OSMData\x18\x03\x0a\x01\xff";
        let blob = osmpbf::BlobReader::new(bytes.as_slice())
            .next()
            .unwrap()
            .unwrap();
        assert!(node_records(&blob).is_err());
    }
}
