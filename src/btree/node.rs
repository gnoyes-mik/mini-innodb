use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

use super::key::Key;

/// 리프 노드의 엔트리: key → value
#[derive(Debug, Clone)]
pub struct LeafEntry {
    pub key: Key,
    pub value: Vec<u8>,
}

/// 내부 노드의 엔트리: key + 자식 페이지 번호
#[derive(Debug, Clone)]
pub struct InternalEntry {
    pub key: Key,
    pub child_page_no: u32,
}

/// B+Tree 노드
///
/// InnoDB에서 B+Tree의 모든 노드는 하나의 페이지에 저장된다.
/// 노드 타입에 따라 내부 노드 또는 리프 노드로 구분한다.
#[derive(Debug)]
pub enum BTreeNode {
    Internal(InternalNode),
    Leaf(LeafNode),
}

/// 내부 노드
///
/// n개의 키와 n+1개의 자식 포인터를 가진다.
/// children[i]는 keys[i-1] 이상 keys[i] 미만인 키를 포함하는 서브트리.
///
/// ```text
///        [  key0  |  key1  ]
///       /         |         \
/// child0     child1       child2
/// (< key0)  (≥key0,<key1) (≥ key1)
/// ```
#[derive(Debug)]
pub struct InternalNode {
    /// 첫 번째 자식 (가장 왼쪽, keys[0]보다 작은 키들)
    pub first_child_page_no: u32,
    /// (key, child_page_no) 쌍의 목록
    pub entries: Vec<InternalEntry>,
}

/// 리프 노드
///
/// 실제 데이터(key-value 쌍)가 저장되는 노드.
/// prev/next로 리프 노드끼리 양방향 연결되어 범위 스캔을 지원한다.
///
/// ```text
/// [Leaf prev=2] ←→ [Leaf (this)] ←→ [Leaf next=5]
///   key1: val1       key3: val3        key5: val5
///   key2: val2       key4: val4        key6: val6
/// ```
#[derive(Debug)]
pub struct LeafNode {
    pub entries: Vec<LeafEntry>,
    pub prev_page_no: Option<u32>,
    pub next_page_no: Option<u32>,
}

/// 노드 타입 마커 (직렬화용)
const NODE_TYPE_INTERNAL: u8 = 1;
const NODE_TYPE_LEAF: u8 = 2;

/// prev/next가 없음을 나타내는 센티넬 값
const PAGE_NO_NONE: u32 = u32::MAX;

impl BTreeNode {
    /// 빈 리프 노드 생성
    pub fn new_leaf() -> Self {
        BTreeNode::Leaf(LeafNode {
            entries: Vec::new(),
            prev_page_no: None,
            next_page_no: None,
        })
    }

    /// 빈 내부 노드 생성
    pub fn new_internal(first_child_page_no: u32) -> Self {
        BTreeNode::Internal(InternalNode {
            first_child_page_no,
            entries: Vec::new(),
        })
    }

    pub fn is_leaf(&self) -> bool {
        matches!(self, BTreeNode::Leaf(_))
    }

    /// 바이트 배열로 직렬화 (페이지의 데이터 영역에 저장용)
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        match self {
            BTreeNode::Leaf(leaf) => {
                buf.push(NODE_TYPE_LEAF);

                // prev/next 페이지 번호
                buf.write_u32::<BigEndian>(
                    leaf.prev_page_no.unwrap_or(PAGE_NO_NONE),
                ).unwrap();
                buf.write_u32::<BigEndian>(
                    leaf.next_page_no.unwrap_or(PAGE_NO_NONE),
                ).unwrap();

                // 엔트리 수
                buf.write_u16::<BigEndian>(leaf.entries.len() as u16).unwrap();

                // 각 엔트리
                for entry in &leaf.entries {
                    entry.key.serialize_into(&mut buf);
                    buf.write_u16::<BigEndian>(entry.value.len() as u16).unwrap();
                    buf.extend_from_slice(&entry.value);
                }
            }
            BTreeNode::Internal(internal) => {
                buf.push(NODE_TYPE_INTERNAL);

                // 첫 번째 자식
                buf.write_u32::<BigEndian>(internal.first_child_page_no).unwrap();

                // 엔트리 수
                buf.write_u16::<BigEndian>(internal.entries.len() as u16).unwrap();

                // 각 엔트리
                for entry in &internal.entries {
                    entry.key.serialize_into(&mut buf);
                    buf.write_u32::<BigEndian>(entry.child_page_no).unwrap();
                }
            }
        }

        buf
    }

    /// 바이트 배열에서 역직렬화
    pub fn deserialize(data: &[u8]) -> Self {
        let node_type = data[0];
        let mut offset = 1;

        match node_type {
            NODE_TYPE_LEAF => {
                let mut cursor = Cursor::new(&data[offset..]);
                let prev = cursor.read_u32::<BigEndian>().unwrap();
                let next = cursor.read_u32::<BigEndian>().unwrap();
                let count = cursor.read_u16::<BigEndian>().unwrap() as usize;
                offset += 4 + 4 + 2;

                let prev_page_no = if prev == PAGE_NO_NONE { None } else { Some(prev) };
                let next_page_no = if next == PAGE_NO_NONE { None } else { Some(next) };

                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let (key, key_size) = Key::deserialize_from(&data[offset..]);
                    offset += key_size;

                    let mut cursor = Cursor::new(&data[offset..]);
                    let val_len = cursor.read_u16::<BigEndian>().unwrap() as usize;
                    offset += 2;

                    let value = data[offset..offset + val_len].to_vec();
                    offset += val_len;

                    entries.push(LeafEntry { key, value });
                }

                BTreeNode::Leaf(LeafNode {
                    entries,
                    prev_page_no,
                    next_page_no,
                })
            }
            NODE_TYPE_INTERNAL => {
                let mut cursor = Cursor::new(&data[offset..]);
                let first_child = cursor.read_u32::<BigEndian>().unwrap();
                let count = cursor.read_u16::<BigEndian>().unwrap() as usize;
                offset += 4 + 2;

                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let (key, key_size) = Key::deserialize_from(&data[offset..]);
                    offset += key_size;

                    let mut cursor = Cursor::new(&data[offset..]);
                    let child_page_no = cursor.read_u32::<BigEndian>().unwrap();
                    offset += 4;

                    entries.push(InternalEntry { key, child_page_no });
                }

                BTreeNode::Internal(InternalNode {
                    first_child_page_no: first_child,
                    entries,
                })
            }
            _ => panic!("unknown node type: {}", node_type),
        }
    }
}

impl InternalNode {
    /// 주어진 키가 속해야 하는 자식 페이지 번호를 찾는다.
    ///
    /// entries: [key0, key1, key2]
    /// children: [first_child, child0, child1, child2]
    ///
    /// key < key0 → first_child
    /// key0 ≤ key < key1 → child0
    /// key1 ≤ key < key2 → child1
    /// key2 ≤ key → child2
    pub fn find_child(&self, key: &Key) -> u32 {
        for entry in self.entries.iter().rev() {
            if *key >= entry.key {
                return entry.child_page_no;
            }
        }
        self.first_child_page_no
    }

    /// 새로운 (key, child) 쌍을 정렬 위치에 삽입
    pub fn insert_entry(&mut self, key: Key, child_page_no: u32) {
        let pos = self.entries.partition_point(|e| e.key < key);
        self.entries.insert(pos, InternalEntry { key, child_page_no });
    }
}

impl LeafNode {
    /// 키로 값을 검색
    pub fn search(&self, key: &Key) -> Option<&[u8]> {
        self.entries
            .binary_search_by(|e| e.key.cmp(key))
            .ok()
            .map(|i| self.entries[i].value.as_slice())
    }

    /// key-value 쌍을 정렬 위치에 삽입. 이미 있으면 값을 업데이트.
    pub fn insert(&mut self, key: Key, value: Vec<u8>) {
        match self.entries.binary_search_by(|e| e.key.cmp(&key)) {
            Ok(i) => {
                // 이미 존재 → 값 업데이트
                self.entries[i].value = value;
            }
            Err(i) => {
                // 새로 삽입
                self.entries.insert(i, LeafEntry { key, value });
            }
        }
    }

    /// 엔트리 수
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leaf_insert_and_search() {
        let mut leaf = LeafNode {
            entries: Vec::new(),
            prev_page_no: None,
            next_page_no: None,
        };

        leaf.insert(Key::from_u64(10), b"hello".to_vec());
        leaf.insert(Key::from_u64(5), b"world".to_vec());
        leaf.insert(Key::from_u64(20), b"foo".to_vec());

        assert_eq!(leaf.search(&Key::from_u64(5)), Some(b"world".as_slice()));
        assert_eq!(leaf.search(&Key::from_u64(10)), Some(b"hello".as_slice()));
        assert_eq!(leaf.search(&Key::from_u64(20)), Some(b"foo".as_slice()));
        assert_eq!(leaf.search(&Key::from_u64(99)), None);
    }

    #[test]
    fn test_leaf_update() {
        let mut leaf = LeafNode {
            entries: Vec::new(),
            prev_page_no: None,
            next_page_no: None,
        };

        leaf.insert(Key::from_u64(10), b"old".to_vec());
        leaf.insert(Key::from_u64(10), b"new".to_vec());

        assert_eq!(leaf.search(&Key::from_u64(10)), Some(b"new".as_slice()));
        assert_eq!(leaf.len(), 1);
    }

    #[test]
    fn test_leaf_sorted_order() {
        let mut leaf = LeafNode {
            entries: Vec::new(),
            prev_page_no: None,
            next_page_no: None,
        };

        leaf.insert(Key::from_u64(30), b"c".to_vec());
        leaf.insert(Key::from_u64(10), b"a".to_vec());
        leaf.insert(Key::from_u64(20), b"b".to_vec());

        let keys: Vec<u64> = leaf.entries.iter()
            .map(|e| e.key.as_u64().unwrap())
            .collect();
        assert_eq!(keys, vec![10, 20, 30]);
    }

    #[test]
    fn test_internal_find_child() {
        let internal = InternalNode {
            first_child_page_no: 100,
            entries: vec![
                InternalEntry { key: Key::from_u64(10), child_page_no: 101 },
                InternalEntry { key: Key::from_u64(20), child_page_no: 102 },
                InternalEntry { key: Key::from_u64(30), child_page_no: 103 },
            ],
        };

        assert_eq!(internal.find_child(&Key::from_u64(5)), 100);   // < 10
        assert_eq!(internal.find_child(&Key::from_u64(10)), 101);  // >= 10, < 20
        assert_eq!(internal.find_child(&Key::from_u64(15)), 101);  // >= 10, < 20
        assert_eq!(internal.find_child(&Key::from_u64(20)), 102);  // >= 20, < 30
        assert_eq!(internal.find_child(&Key::from_u64(30)), 103);  // >= 30
        assert_eq!(internal.find_child(&Key::from_u64(99)), 103);  // >= 30
    }

    #[test]
    fn test_serialize_deserialize_leaf() {
        let mut leaf = LeafNode {
            entries: Vec::new(),
            prev_page_no: Some(3),
            next_page_no: Some(7),
        };
        leaf.insert(Key::from_u64(10), b"hello".to_vec());
        leaf.insert(Key::from_u64(20), b"world".to_vec());

        let node = BTreeNode::Leaf(leaf);
        let bytes = node.serialize();
        let restored = BTreeNode::deserialize(&bytes);

        if let BTreeNode::Leaf(leaf) = restored {
            assert_eq!(leaf.prev_page_no, Some(3));
            assert_eq!(leaf.next_page_no, Some(7));
            assert_eq!(leaf.entries.len(), 2);
            assert_eq!(leaf.search(&Key::from_u64(10)), Some(b"hello".as_slice()));
            assert_eq!(leaf.search(&Key::from_u64(20)), Some(b"world".as_slice()));
        } else {
            panic!("expected leaf node");
        }
    }

    #[test]
    fn test_serialize_deserialize_internal() {
        let node = BTreeNode::Internal(InternalNode {
            first_child_page_no: 100,
            entries: vec![
                InternalEntry { key: Key::from_u64(10), child_page_no: 101 },
                InternalEntry { key: Key::from_u64(20), child_page_no: 102 },
            ],
        });

        let bytes = node.serialize();
        let restored = BTreeNode::deserialize(&bytes);

        if let BTreeNode::Internal(internal) = restored {
            assert_eq!(internal.first_child_page_no, 100);
            assert_eq!(internal.entries.len(), 2);
            assert_eq!(internal.find_child(&Key::from_u64(5)), 100);
            assert_eq!(internal.find_child(&Key::from_u64(15)), 101);
            assert_eq!(internal.find_child(&Key::from_u64(25)), 102);
        } else {
            panic!("expected internal node");
        }
    }
}
