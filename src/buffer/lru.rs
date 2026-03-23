use std::collections::HashMap;

/// LRU (Least Recently Used) 리스트
///
/// InnoDB 버퍼 풀에서 페이지 교체 대상을 결정하는 자료구조.
/// 가장 최근에 사용된 항목은 head 쪽, 가장 오래된 항목은 tail 쪽에 위치한다.
/// 교체가 필요하면 tail에서 꺼낸다.
///
/// 이중 연결 리스트 + HashMap으로 O(1) 접근/이동/제거를 구현한다.
pub struct LruList {
    /// 실제 노드 저장소 (frame_id → node)
    entries: HashMap<usize, LruNode>,
    head: Option<usize>,
    tail: Option<usize>,
}

struct LruNode {
    prev: Option<usize>,
    next: Option<usize>,
}

impl LruList {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            head: None,
            tail: None,
        }
    }

    /// 항목을 head로 이동 (가장 최근 사용으로 표시)
    /// 없으면 새로 추가
    pub fn touch(&mut self, frame_id: usize) {
        if self.entries.contains_key(&frame_id) {
            self.remove(frame_id);
        }
        self.push_front(frame_id);
    }

    /// tail에서 가장 오래된 항목을 제거하고 반환 (교체 대상)
    pub fn evict(&mut self) -> Option<usize> {
        let tail_id = self.tail?;
        self.remove(tail_id);
        Some(tail_id)
    }

    /// 특정 항목 제거
    pub fn remove(&mut self, frame_id: usize) {
        let node = match self.entries.remove(&frame_id) {
            Some(n) => n,
            None => return,
        };

        // prev와 next를 연결
        match (node.prev, node.next) {
            (Some(prev), Some(next)) => {
                self.entries.get_mut(&prev).unwrap().next = Some(next);
                self.entries.get_mut(&next).unwrap().prev = Some(prev);
            }
            (Some(prev), None) => {
                // node가 tail이었음
                self.entries.get_mut(&prev).unwrap().next = None;
                self.tail = Some(prev);
            }
            (None, Some(next)) => {
                // node가 head였음
                self.entries.get_mut(&next).unwrap().prev = None;
                self.head = Some(next);
            }
            (None, None) => {
                // 유일한 노드였음
                self.head = None;
                self.tail = None;
            }
        }
    }

    /// head에 새 항목 추가
    fn push_front(&mut self, frame_id: usize) {
        let new_node = LruNode {
            prev: None,
            next: self.head,
        };

        if let Some(old_head) = self.head {
            self.entries.get_mut(&old_head).unwrap().prev = Some(frame_id);
        }

        self.head = Some(frame_id);
        if self.tail.is_none() {
            self.tail = Some(frame_id);
        }

        self.entries.insert(frame_id, new_node);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_evict_order() {
        let mut lru = LruList::new();
        // 삽입 순서: 0, 1, 2
        lru.touch(0);
        lru.touch(1);
        lru.touch(2);

        // evict 순서: 가장 오래된 순 → 0, 1, 2
        assert_eq!(lru.evict(), Some(0));
        assert_eq!(lru.evict(), Some(1));
        assert_eq!(lru.evict(), Some(2));
        assert_eq!(lru.evict(), None);
    }

    #[test]
    fn test_touch_moves_to_front() {
        let mut lru = LruList::new();
        lru.touch(0);
        lru.touch(1);
        lru.touch(2);

        // 0을 다시 touch → head로 이동
        lru.touch(0);

        // evict 순서: 1, 2, 0
        assert_eq!(lru.evict(), Some(1));
        assert_eq!(lru.evict(), Some(2));
        assert_eq!(lru.evict(), Some(0));
    }

    #[test]
    fn test_remove() {
        let mut lru = LruList::new();
        lru.touch(0);
        lru.touch(1);
        lru.touch(2);

        lru.remove(1);
        assert_eq!(lru.len(), 2);

        assert_eq!(lru.evict(), Some(0));
        assert_eq!(lru.evict(), Some(2));
    }

    #[test]
    fn test_single_element() {
        let mut lru = LruList::new();
        lru.touch(42);
        assert_eq!(lru.len(), 1);

        lru.touch(42); // 다시 touch
        assert_eq!(lru.len(), 1);

        assert_eq!(lru.evict(), Some(42));
        assert!(lru.is_empty());
    }
}
