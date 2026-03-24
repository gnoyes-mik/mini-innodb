use crate::buffer::{BufferPool, BufferPoolError};
use crate::page::{PageId, PageType};
#[cfg(test)]
use crate::page::FileManager;
use super::key::Key;
use super::node::{BTreeNode, LeafNode, InternalNode};

/// 한 리프 노드의 최대 엔트리 수
/// 실제 InnoDB는 페이지 크기와 레코드 크기에 따라 동적으로 결정되지만,
/// 학습 목적으로 고정값을 사용한다.
const MAX_LEAF_ENTRIES: usize = 4;

/// 한 내부 노드의 최대 엔트리 수
const MAX_INTERNAL_ENTRIES: usize = 4;

/// 디스크 기반 B+Tree 인덱스
///
/// InnoDB에서 테이블 = Clustered Index = B+Tree.
/// 모든 행 데이터가 리프 노드에 저장된다.
pub struct BTree {
    root_page_no: u32,
    buffer_pool: BufferPool,
}

#[derive(Debug, thiserror::Error)]
pub enum BTreeError {
    #[error("buffer pool error: {0}")]
    BufferPool(#[from] BufferPoolError),
    #[error("page data error: {0}")]
    PageData(#[from] crate::page::PageError),
}

impl BTree {
    /// 새 B+Tree 생성 (빈 리프 노드를 root로)
    pub fn new(buffer_pool: BufferPool) -> Result<Self, BTreeError> {
        let root_page_no;

        // root 페이지 할당 및 빈 리프 노드로 초기화
        {
            let guard = buffer_pool.new_page(PageType::Index)?;
            root_page_no = guard.read(|page| page.page_no());

            let node = BTreeNode::new_leaf();
            let bytes = node.serialize();
            guard.write(|page| {
                page.write_data(0, &bytes).unwrap();
            });
        }

        Ok(Self {
            root_page_no,
            buffer_pool,
        })
    }

    /// 키로 값을 검색
    pub fn search(&self, key: &Key) -> Result<Option<Vec<u8>>, BTreeError> {
        let leaf_page_no = self.find_leaf(key)?;
        let guard = self.buffer_pool.fetch_page(PageId::new(0, leaf_page_no))?;

        let result = guard.read(|page| {
            let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
            let node = BTreeNode::deserialize(data);
            if let BTreeNode::Leaf(leaf) = node {
                leaf.search(key).map(|v| v.to_vec())
            } else {
                None
            }
        });

        Ok(result)
    }

    /// key-value 쌍 삽입
    pub fn insert(&mut self, key: Key, value: Vec<u8>) -> Result<(), BTreeError> {
        let result = self.insert_recursive(self.root_page_no, key, value)?;

        // 루트가 분할되었으면 새 루트 생성
        if let Some((split_key, new_page_no)) = result {
            let old_root = self.root_page_no;

            // 새 루트 페이지 할당
            let guard = self.buffer_pool.new_page(PageType::Index)?;
            let new_root_page_no = guard.read(|page| page.page_no());

            let mut new_root = InternalNode {
                first_child_page_no: old_root,
                entries: Vec::new(),
            };
            new_root.insert_entry(split_key, new_page_no);

            let bytes = BTreeNode::Internal(new_root).serialize();
            guard.write(|page| {
                page.write_data(0, &bytes).unwrap();
            });

            self.root_page_no = new_root_page_no;
        }

        Ok(())
    }

    /// 범위 검색: start_key 이상 end_key 이하의 모든 엔트리 반환
    pub fn range_scan(&self, start_key: &Key, end_key: &Key) -> Result<Vec<(Key, Vec<u8>)>, BTreeError> {
        let mut results = Vec::new();
        let mut current_page_no = self.find_leaf(start_key)?;

        'outer: loop {
            let (entries, next_page_no) = {
                let guard = self.buffer_pool.fetch_page(PageId::new(0, current_page_no))?;
                guard.read(|page| {
                    let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                    let node = BTreeNode::deserialize(data);
                    if let BTreeNode::Leaf(leaf) = node {
                        let entries: Vec<(Key, Vec<u8>)> = leaf.entries.iter()
                            .filter(|e| e.key >= *start_key && e.key <= *end_key)
                            .map(|e| (e.key.clone(), e.value.clone()))
                            .collect();
                        let next = leaf.next_page_no;
                        (entries, next)
                    } else {
                        (Vec::new(), None)
                    }
                })
            };

            for entry in entries {
                if entry.0 > *end_key {
                    break 'outer;
                }
                results.push(entry);
            }

            match next_page_no {
                Some(next) => {
                    // 다음 리프의 첫 키가 end_key를 넘으면 중단
                    current_page_no = next;
                }
                None => break,
            }
        }

        Ok(results)
    }

    /// root에서 리프까지 탐색하여 해당 키가 속하는 리프 페이지 번호 반환
    fn find_leaf(&self, key: &Key) -> Result<u32, BTreeError> {
        let mut current_page_no = self.root_page_no;

        loop {
            let guard = self.buffer_pool.fetch_page(PageId::new(0, current_page_no))?;
            let next = guard.read(|page| {
                let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                let node = BTreeNode::deserialize(data);
                match node {
                    BTreeNode::Leaf(_) => None, // 리프에 도착
                    BTreeNode::Internal(internal) => Some(internal.find_child(key)),
                }
            });

            match next {
                Some(child_page_no) => current_page_no = child_page_no,
                None => return Ok(current_page_no),
            }
        }
    }

    /// 재귀적 삽입. 분할이 발생하면 (분할 키, 새 페이지 번호)를 반환.
    fn insert_recursive(
        &mut self,
        page_no: u32,
        key: Key,
        value: Vec<u8>,
    ) -> Result<Option<(Key, u32)>, BTreeError> {
        let guard = self.buffer_pool.fetch_page(PageId::new(0, page_no))?;
        let is_leaf = guard.read(|page| {
            let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
            BTreeNode::deserialize(data).is_leaf()
        });

        if is_leaf {
            // 리프 노드에 삽입
            let needs_split = guard.read(|page| {
                let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                if let BTreeNode::Leaf(leaf) = BTreeNode::deserialize(data) {
                    // 이미 같은 키가 있으면 업데이트이므로 분할 불필요
                    let exists = leaf.search(&key).is_some();
                    !exists && leaf.len() >= MAX_LEAF_ENTRIES
                } else {
                    false
                }
            });

            if needs_split {
                // 분할 필요 — 먼저 현재 노드에 삽입 후 분할
                drop(guard);
                return self.split_leaf(page_no, key, value);
            }

            // 분할 불필요 — 그냥 삽입
            guard.write(|page| {
                let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                let mut node = BTreeNode::deserialize(data);
                if let BTreeNode::Leaf(ref mut leaf) = node {
                    leaf.insert(key, value);
                }
                let bytes = node.serialize();
                page.write_data(0, &bytes).unwrap();
            });

            Ok(None)
        } else {
            // 내부 노드 — 자식으로 재귀
            let child_page_no = guard.read(|page| {
                let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                if let BTreeNode::Internal(internal) = BTreeNode::deserialize(data) {
                    internal.find_child(&key)
                } else {
                    unreachable!()
                }
            });
            drop(guard);

            let child_result = self.insert_recursive(child_page_no, key, value)?;

            // 자식이 분할되었으면 이 노드에 새 엔트리 추가
            if let Some((split_key, new_child_page_no)) = child_result {
                let guard = self.buffer_pool.fetch_page(PageId::new(0, page_no))?;

                let needs_split = guard.read(|page| {
                    let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                    if let BTreeNode::Internal(internal) = BTreeNode::deserialize(data) {
                        internal.entries.len() >= MAX_INTERNAL_ENTRIES
                    } else {
                        false
                    }
                });

                if needs_split {
                    drop(guard);
                    return self.split_internal(page_no, split_key, new_child_page_no);
                }

                guard.write(|page| {
                    let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                    let mut node = BTreeNode::deserialize(data);
                    if let BTreeNode::Internal(ref mut internal) = node {
                        internal.insert_entry(split_key, new_child_page_no);
                    }
                    let bytes = node.serialize();
                    page.write_data(0, &bytes).unwrap();
                });

                Ok(None)
            } else {
                Ok(None)
            }
        }
    }

    /// 리프 노드 분할
    ///
    /// 1. 현재 리프에 새 엔트리를 삽입한 상태에서 절반을 새 페이지로 이동
    /// 2. prev/next 포인터 갱신
    /// 3. (분할 키, 새 페이지 번호) 반환
    fn split_leaf(
        &mut self,
        page_no: u32,
        key: Key,
        value: Vec<u8>,
    ) -> Result<Option<(Key, u32)>, BTreeError> {
        // 현재 리프의 모든 엔트리를 읽어서 새 키 포함한 전체 목록 만들기
        let guard = self.buffer_pool.fetch_page(PageId::new(0, page_no))?;
        let (mut all_entries, old_next) = guard.read(|page| {
            let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
            if let BTreeNode::Leaf(mut leaf) = BTreeNode::deserialize(data) {
                leaf.insert(key, value);
                let next = leaf.next_page_no;
                (leaf.entries, next)
            } else {
                unreachable!()
            }
        });

        // 절반으로 분할
        let mid = all_entries.len() / 2;
        let right_entries: Vec<_> = all_entries.drain(mid..).collect();
        let left_entries = all_entries;
        let split_key = right_entries[0].key.clone();

        // 새 리프 페이지 할당
        let new_guard = self.buffer_pool.new_page(PageType::Index)?;
        let new_page_no = new_guard.read(|page| page.page_no());

        // 새 리프 (오른쪽) 작성
        let right_leaf = BTreeNode::Leaf(LeafNode {
            entries: right_entries,
            prev_page_no: Some(page_no),
            next_page_no: old_next,
        });
        let bytes = right_leaf.serialize();
        new_guard.write(|page| {
            page.write_data(0, &bytes).unwrap();
        });
        drop(new_guard);

        // 기존 리프 (왼쪽) 갱신
        let left_leaf = BTreeNode::Leaf(LeafNode {
            entries: left_entries,
            prev_page_no: {
                guard.read(|page| {
                    let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                    if let BTreeNode::Leaf(leaf) = BTreeNode::deserialize(data) {
                        leaf.prev_page_no
                    } else {
                        None
                    }
                })
            },
            next_page_no: Some(new_page_no),
        });
        let bytes = left_leaf.serialize();
        guard.write(|page| {
            page.write_data(0, &bytes).unwrap();
        });
        drop(guard);

        // old_next의 prev 포인터 갱신
        if let Some(next_page_no) = old_next {
            let next_guard = self.buffer_pool.fetch_page(PageId::new(0, next_page_no))?;
            next_guard.write(|page| {
                let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
                if let BTreeNode::Leaf(mut leaf) = BTreeNode::deserialize(data) {
                    leaf.prev_page_no = Some(new_page_no);
                    let bytes = BTreeNode::Leaf(leaf).serialize();
                    page.write_data(0, &bytes).unwrap();
                }
            });
        }

        Ok(Some((split_key, new_page_no)))
    }

    /// 내부 노드 분할
    fn split_internal(
        &mut self,
        page_no: u32,
        new_key: Key,
        new_child_page_no: u32,
    ) -> Result<Option<(Key, u32)>, BTreeError> {
        let guard = self.buffer_pool.fetch_page(PageId::new(0, page_no))?;
        let (first_child, mut all_entries) = guard.read(|page| {
            let data = page.read_data(0, crate::page::Page::DATA_END - crate::page::Page::DATA_START).unwrap();
            if let BTreeNode::Internal(mut internal) = BTreeNode::deserialize(data) {
                internal.insert_entry(new_key, new_child_page_no);
                (internal.first_child_page_no, internal.entries)
            } else {
                unreachable!()
            }
        });

        let mid = all_entries.len() / 2;
        let right_entries: Vec<_> = all_entries.drain(mid + 1..).collect();
        let split_entry = all_entries.pop().unwrap(); // mid번째가 상위로 올라감
        let left_entries = all_entries;

        // 새 내부 노드 (오른쪽)
        let new_guard = self.buffer_pool.new_page(PageType::Index)?;
        let new_page_no = new_guard.read(|page| page.page_no());

        let right_internal = BTreeNode::Internal(InternalNode {
            first_child_page_no: split_entry.child_page_no,
            entries: right_entries,
        });
        let bytes = right_internal.serialize();
        new_guard.write(|page| {
            page.write_data(0, &bytes).unwrap();
        });
        drop(new_guard);

        // 기존 내부 노드 (왼쪽) 갱신
        let left_internal = BTreeNode::Internal(InternalNode {
            first_child_page_no: first_child,
            entries: left_entries,
        });
        let bytes = left_internal.serialize();
        guard.write(|page| {
            page.write_data(0, &bytes).unwrap();
        });
        drop(guard);

        Ok(Some((split_entry.key, new_page_no)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_btree(pool_size: usize) -> (BTree, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let fm = FileManager::open_or_create(0, tmp.path()).unwrap();
        let pool = BufferPool::new(pool_size, fm);
        let btree = BTree::new(pool).unwrap();
        (btree, tmp)
    }

    #[test]
    fn test_insert_and_search() {
        let (mut btree, _tmp) = setup_btree(100);

        btree.insert(Key::from_u64(10), b"ten".to_vec()).unwrap();
        btree.insert(Key::from_u64(20), b"twenty".to_vec()).unwrap();
        btree.insert(Key::from_u64(5), b"five".to_vec()).unwrap();

        assert_eq!(btree.search(&Key::from_u64(10)).unwrap(), Some(b"ten".to_vec()));
        assert_eq!(btree.search(&Key::from_u64(20)).unwrap(), Some(b"twenty".to_vec()));
        assert_eq!(btree.search(&Key::from_u64(5)).unwrap(), Some(b"five".to_vec()));
        assert_eq!(btree.search(&Key::from_u64(99)).unwrap(), None);
    }

    #[test]
    fn test_update_existing_key() {
        let (mut btree, _tmp) = setup_btree(100);

        btree.insert(Key::from_u64(10), b"old".to_vec()).unwrap();
        btree.insert(Key::from_u64(10), b"new".to_vec()).unwrap();

        assert_eq!(btree.search(&Key::from_u64(10)).unwrap(), Some(b"new".to_vec()));
    }

    #[test]
    fn test_leaf_split() {
        let (mut btree, _tmp) = setup_btree(100);

        // MAX_LEAF_ENTRIES = 4이므로 5번째 삽입에서 분할 발생
        for i in 1..=6 {
            let val = format!("val-{}", i);
            btree.insert(Key::from_u64(i), val.into_bytes()).unwrap();
        }

        // 분할 후에도 모든 키 검색 가능
        for i in 1..=6 {
            let expected = format!("val-{}", i);
            assert_eq!(
                btree.search(&Key::from_u64(i)).unwrap(),
                Some(expected.into_bytes())
            );
        }
    }

    #[test]
    fn test_many_inserts() {
        let (mut btree, _tmp) = setup_btree(200);

        // 여러 번의 분할을 유발하는 대량 삽입
        for i in 1..=50 {
            let val = format!("value-{}", i);
            btree.insert(Key::from_u64(i), val.into_bytes()).unwrap();
        }

        // 모든 키가 올바르게 검색되는지 확인
        for i in 1..=50 {
            let expected = format!("value-{}", i);
            assert_eq!(
                btree.search(&Key::from_u64(i)).unwrap(),
                Some(expected.into_bytes()),
                "key {} not found",
                i
            );
        }
    }

    #[test]
    fn test_reverse_order_inserts() {
        let (mut btree, _tmp) = setup_btree(200);

        // 역순 삽입
        for i in (1..=20).rev() {
            let val = format!("val-{}", i);
            btree.insert(Key::from_u64(i), val.into_bytes()).unwrap();
        }

        for i in 1..=20 {
            let expected = format!("val-{}", i);
            assert_eq!(
                btree.search(&Key::from_u64(i)).unwrap(),
                Some(expected.into_bytes())
            );
        }
    }

    #[test]
    fn test_range_scan() {
        let (mut btree, _tmp) = setup_btree(100);

        for i in 1..=10 {
            let val = format!("v{}", i);
            btree.insert(Key::from_u64(i), val.into_bytes()).unwrap();
        }

        let results = btree.range_scan(&Key::from_u64(3), &Key::from_u64(7)).unwrap();
        let keys: Vec<u64> = results.iter().map(|(k, _)| k.as_u64().unwrap()).collect();
        assert_eq!(keys, vec![3, 4, 5, 6, 7]);
    }

    #[test]
    fn test_range_scan_all() {
        let (mut btree, _tmp) = setup_btree(100);

        for i in 1..=10 {
            btree.insert(Key::from_u64(i), vec![i as u8]).unwrap();
        }

        let results = btree.range_scan(&Key::from_u64(1), &Key::from_u64(10)).unwrap();
        assert_eq!(results.len(), 10);
    }
}
