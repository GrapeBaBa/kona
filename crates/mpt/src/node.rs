//! This module contains the [TrieNode] type, which represents a node within a standard Merkle
//! Patricia Trie.

use alloc::{boxed::Box, vec, vec::Vec};
use alloy_primitives::{keccak256, Bytes, B256};
use alloy_rlp::{Buf, BufMut, Decodable, Encodable, Header, EMPTY_STRING_CODE};
use anyhow::{anyhow, Result};

/// The length of the branch list when RLP encoded
const BRANCH_LIST_LENGTH: usize = 17;

/// The length of a leaf or extension node's RLP encoded list
const LEAF_OR_EXTENSION_LIST_LENGTH: usize = 2;

/// Prefix for even-nibbled extension node paths.
const PREFIX_EXTENSION_EVEN: u8 = 0;

/// Prefix for odd-nibbled extension node paths.
const PREFIX_EXTENSION_ODD: u8 = 1;

/// Prefix for even-nibbled leaf node paths.
const PREFIX_LEAF_EVEN: u8 = 2;

/// Prefix for odd-nibbled leaf node paths.
const PREFIX_LEAF_ODD: u8 = 3;

/// A [TrieNode] is a node within a standard Ethereum Merkle Patricia Trie.
///
/// The [TrieNode] has several variants:
/// - [TrieNode::Empty] represents an empty node.
/// - [TrieNode::Blinded] represents a node that has been blinded by a commitment.
/// - [TrieNode::Leaf] represents a 2-item node with the encoding `rlp([encoded_path, value])`.
/// - [TrieNode::Extension] represents a 2-item pointer node with the encoding `rlp([encoded_path,
///   key])`.
/// - [TrieNode::Branch] represents a node that refers to up to 16 child nodes with the encoding
///   `rlp([ v0, ..., v15, value ])`.
///
/// In the Ethereum Merkle Patricia Trie, nodes longer than an encoded 32 byte string (33 total
/// bytes) are blinded with [keccak256] hashes. When a node is "opened", it is replaced with the
/// [TrieNode] that is decoded from to the preimage of the hash.
///
/// The [alloy_rlp::Encodable] and [alloy_rlp::Decodable] traits are implemented for [TrieNode],
/// allowing for RLP encoding and decoding of the types for storage and retrieval. The
/// implementation of these traits will implicitly blind nodes that are longer than 32 bytes in
/// length when encoding. When decoding, the implementation will leave blinded nodes in place.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TrieNode {
    /// An empty [TrieNode] is represented as an [EMPTY_STRING_CODE] (0x80).
    Empty,
    /// A blinded node is a node that has been blinded by a [keccak256] commitment.
    Blinded {
        /// The commitment that blinds the node.
        commitment: B256,
    },
    /// A leaf node is a 2-item node with the encoding `rlp([encoded_path, value])`
    Leaf {
        /// The key of the leaf node
        key: Bytes,
        /// The value of the leaf node
        value: Bytes,
    },
    /// An extension node is a 2-item pointer node with the encoding `rlp([encoded_path, key])`
    Extension {
        /// The path prefix of the extension
        prefix: Bytes,
        /// The pointer to the child node
        node: Box<TrieNode>,
    },
    /// A branch node refers to up to 16 child nodes with the encoding
    /// `rlp([ v0, ..., v15, value ])`
    Branch {
        /// The 16 child nodes and value of the branch.
        stack: Vec<TrieNode>,
    },
}

impl TrieNode {
    /// Attempts to convert a `path` and `value` into a [TrieNode], if they correspond to a
    /// [TrieNode::Leaf] or [TrieNode::Extension].
    ///
    /// **Note:** This function assumes that the passed reader has already consumed the RLP header
    /// of the [TrieNode::Leaf] or [TrieNode::Extension] node.
    pub fn try_decode_leaf_or_extension_payload(buf: &mut &[u8]) -> Result<Self> {
        // Decode the path and value of the leaf or extension node.
        let path = Bytes::decode(buf).map_err(|e| anyhow!("Failed to decode: {e}"))?;

        // Check the high-order nibble of the path to determine the type of node.
        match path[0] >> 4 {
            PREFIX_EXTENSION_EVEN | PREFIX_EXTENSION_ODD => {
                // extension node
                let extension_node_value =
                    TrieNode::decode(buf).map_err(|e| anyhow!("Failed to decode: {e}"))?;
                Ok(TrieNode::Extension { prefix: path, node: Box::new(extension_node_value) })
            }
            PREFIX_LEAF_EVEN | PREFIX_LEAF_ODD => {
                // leaf node
                let value = Bytes::decode(buf).map_err(|e| anyhow!("Failed to decode: {e}"))?;
                Ok(TrieNode::Leaf { key: path, value })
            }
            _ => {
                anyhow::bail!("Unexpected path identifier in high-order nibble")
            }
        }
    }

    /// Blinds the [TrieNode] if it is longer than an encoded [B256] string in length, and returns
    /// the mutated node.
    pub fn blind(self) -> Self {
        if self.length() > B256::ZERO.length() {
            let mut rlp_buf = Vec::with_capacity(self.length());
            self.encode(&mut rlp_buf);
            TrieNode::Blinded { commitment: keccak256(rlp_buf) }
        } else {
            self
        }
    }
}

impl Encodable for TrieNode {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Empty => out.put_u8(EMPTY_STRING_CODE),
            Self::Blinded { commitment } => commitment.encode(out),
            Self::Leaf { key, value } => {
                // Encode the leaf node's header and key-value pair.
                let leaf_list = vec![key, value];
                leaf_list.encode(out);
            }
            Self::Extension { prefix, node } => {
                // Encode the extension node's header, prefix, and pointer node.
                Header { list: true, payload_length: prefix.length() + node.length() }.encode(out);
                prefix.encode(out);
                encode_blinded(node.as_ref(), out);
            }
            Self::Branch { stack } => {
                // In branch nodes, if an element is longer than 32 bytes in length, it is blinded.
                // Assuming we have an open trie node, we must re-hash the elements
                // that are longer than 32 bytes in length.
                let blinded_nodes =
                    stack.iter().cloned().map(|node| node.blind()).collect::<Vec<TrieNode>>();
                blinded_nodes.encode(out);
            }
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::Empty => 1,
            Self::Blinded { commitment } => commitment.length(),
            Self::Leaf { key, value } => {
                let leaf_list = vec![key, value];
                leaf_list.length()
            }
            Self::Extension { prefix, node } => {
                let prefix_length = prefix.length();
                let node_length = blinded_length(node.as_ref());
                Header { list: true, payload_length: prefix_length + node_length }.length() +
                    prefix_length +
                    node_length
            }
            Self::Branch { stack } => {
                // In branch nodes, if an element is longer than an encoded 32 byte string, it is
                // blinded. Assuming we have an open trie node, we must re-hash the
                // elements that are longer than an encoded 32 byte string
                // in length.
                let inner_length = stack.iter().fold(0, |mut acc, node| {
                    acc += blinded_length(node);
                    acc
                });

                inner_length + Header { list: true, payload_length: inner_length }.length()
            }
        }
    }
}

impl Decodable for TrieNode {
    /// Attempts to decode the [TrieNode].
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // Peek at the header to determine the type of Trie node we're currently decoding.
        let header = Header::decode(&mut (**buf).as_ref())?;

        if header.list {
            // Peek at the RLP stream to determine the number of elements in the list.
            let list_length = rlp_list_element_length(&mut (**buf).as_ref())?;

            match list_length {
                BRANCH_LIST_LENGTH => {
                    let list = Vec::<TrieNode>::decode(buf)?;
                    Ok(Self::Branch { stack: list })
                }
                LEAF_OR_EXTENSION_LIST_LENGTH => {
                    // Advance the buffer to the start of the list payload.
                    buf.advance(header.length());
                    // Decode the leaf or extension node's raw payload.
                    Self::try_decode_leaf_or_extension_payload(buf)
                        .map_err(|_| alloy_rlp::Error::UnexpectedList)
                }
                _ => Err(alloy_rlp::Error::UnexpectedLength),
            }
        } else {
            match header.payload_length {
                0 => {
                    buf.advance(header.length());
                    Ok(Self::Empty)
                }
                _ => {
                    if header.payload_length != B256::len_bytes() {
                        return Err(alloy_rlp::Error::UnexpectedLength);
                    }
                    let commitment = B256::decode(buf)?;

                    Ok(Self::Blinded { commitment })
                }
            }
        }
    }
}

/// Returns the encoded length of an [Encodable] value, blinding it if it is longer than an encoded
/// [B256] string in length.
fn blinded_length<T: Encodable>(value: T) -> usize {
    if value.length() > B256::ZERO.length() {
        B256::ZERO.length()
    } else {
        value.length()
    }
}

/// Encodes a value into an RLP stream, blidning it with a [keccak256] commitment if it is longer
/// than an encoded [B256] string in length.
fn encode_blinded<T: Encodable>(value: T, out: &mut dyn BufMut) {
    if value.length() > B256::ZERO.length() {
        let mut rlp_buf = Vec::with_capacity(value.length());
        value.encode(&mut rlp_buf);
        TrieNode::Blinded { commitment: keccak256(rlp_buf) }.encode(out);
    } else {
        value.encode(out);
    }
}

/// Walks through a RLP list's elements and returns the total number of elements in the list.
/// Returns [alloy_rlp::Error::UnexpectedString] if the RLP stream is not a list.
fn rlp_list_element_length(buf: &mut &[u8]) -> alloy_rlp::Result<usize> {
    let header = Header::decode(buf)?;
    if !header.list {
        return Err(alloy_rlp::Error::UnexpectedString);
    }
    let len_after_consume = buf.len() - header.payload_length;

    let mut list_element_length = 0;
    while buf.len() > len_after_consume {
        let header = Header::decode(buf)?;
        buf.advance(header.payload_length);
        list_element_length += 1;
    }
    Ok(list_element_length)
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;
    use alloy_primitives::{b256, bytes, hex};

    #[test]
    fn test_decode_branch() {
        const BRANCH_RLP: [u8; 64] = hex!("f83ea0eb08a66a94882454bec899d3e82952dcc918ba4b35a09a84acd98019aef4345080808080808080cd308b8a746573742074687265658080808080808080");
        let expected = TrieNode::Branch {
            stack: vec![
                TrieNode::Blinded {
                    commitment: b256!(
                        "eb08a66a94882454bec899d3e82952dcc918ba4b35a09a84acd98019aef43450"
                    ),
                },
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Leaf { key: bytes!("30"), value: bytes!("8a74657374207468726565") },
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
                TrieNode::Empty,
            ],
        };

        let mut rlp_buf = Vec::with_capacity(expected.length());
        expected.encode(&mut rlp_buf);
        assert_eq!(rlp_buf.len(), BRANCH_RLP.len());
        assert_eq!(expected.length(), BRANCH_RLP.len());

        assert_eq!(expected, TrieNode::decode(&mut BRANCH_RLP.as_slice()).unwrap());
        assert_eq!(rlp_buf.as_slice(), &BRANCH_RLP[..]);
    }

    #[test]
    fn test_encode_decode_extension_open_short() {
        const EXTENSION_RLP: [u8; 19] = hex!("d28300646fcd308b8a74657374207468726565");

        let opened = TrieNode::Leaf { key: bytes!("30"), value: bytes!("8a74657374207468726565") };
        let expected = TrieNode::Extension { prefix: bytes!("00646f"), node: Box::new(opened) };

        let mut rlp_buf = Vec::with_capacity(expected.length());
        expected.encode(&mut rlp_buf);

        assert_eq!(expected, TrieNode::decode(&mut EXTENSION_RLP.as_slice()).unwrap());
    }

    #[test]
    fn test_encode_decode_extension_blinded_long() {
        const EXTENSION_RLP: [u8; 38] =
            hex!("e58300646fa0f3fe8b3c5b21d3e52860f1e4a5825a6100bb341069c1e88f4ebf6bd98de0c190");
        let mut rlp_buf = Vec::new();

        let opened = TrieNode::Leaf { key: bytes!("30"), value: bytes!("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF") };
        opened.encode(&mut rlp_buf);
        let blinded = TrieNode::Blinded { commitment: keccak256(&rlp_buf) };

        rlp_buf.clear();
        let opened_extension =
            TrieNode::Extension { prefix: bytes!("00646f"), node: Box::new(opened) };
        opened_extension.encode(&mut rlp_buf);

        let expected = TrieNode::Extension { prefix: bytes!("00646f"), node: Box::new(blinded) };
        assert_eq!(expected, TrieNode::decode(&mut EXTENSION_RLP.as_slice()).unwrap());
    }

    #[test]
    fn test_decode_leaf() {
        const LEAF_RLP: [u8; 11] = hex!("ca8320646f8576657262FF");
        let expected = TrieNode::Leaf { key: bytes!("20646f"), value: bytes!("76657262FF") };
        assert_eq!(expected, TrieNode::decode(&mut LEAF_RLP.as_slice()).unwrap());
    }
}
