use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use crate::page::{FileManager, Page, PageId, PageType};
use super::lru::LruList;

/// 버퍼 프레임 — 버퍼 풀 내의 페이지 슬롯 하나
///
/// 디스크에서 읽어온 페이지를 메모리에 보관하면서,
/// dirty 여부와 pin count를 함께 관리한다.
pub struct BufferFrame {
    /// 메모리에 올라온 페이지
    page: Page,
    /// 디스크 기록 후 수정되었는지 여부
    is_dirty: bool,
    /// 현재 이 페이지를 사용 중인 횟수 (pin_count > 0이면 교체 불가)
    pin_count: u32,
}

impl BufferFrame {
    fn new(page: Page) -> Self {
        Self {
            page,
            is_dirty: false,
            pin_count: 0,
        }
    }
}

/// 버퍼 풀
///
/// InnoDB에서 가장 중요한 메모리 구조체.
/// 디스크 페이지를 메모리에 캐싱하여 디스크 I/O를 최소화한다.
///
/// 핵심 동작:
/// 1. 페이지 요청 → page_table에서 검색
/// 2. 캐시 히트 → 바로 반환, LRU에서 head로 이동
/// 3. 캐시 미스 → 빈 프레임 확보 (필요시 evict) → 디스크에서 읽어서 적재
pub struct BufferPool {
    /// 최대 프레임 수
    capacity: usize,
    /// PageId → frame index 매핑
    page_table: Mutex<HashMap<PageId, usize>>,
    /// 실제 프레임 배열 (각 프레임은 독립적으로 잠금 가능)
    frames: Vec<RwLock<Option<BufferFrame>>>,
    /// LRU 리스트 — 교체 대상 선정
    lru: Mutex<LruList>,
    /// 사용 가능한 빈 프레임 인덱스
    free_list: Mutex<Vec<usize>>,
    /// 디스크 I/O 담당
    file_manager: Mutex<FileManager>,
}

/// 페이지 접근을 위한 RAII 가드
///
/// 가드가 drop되면 자동으로 pin_count가 감소한다.
/// Java의 try-with-resources와 유사한 패턴.
pub struct PageGuard<'a> {
    pool: &'a BufferPool,
    frame_idx: usize,
}

impl<'a> PageGuard<'a> {
    /// 페이지를 읽기 전용으로 접근
    pub fn read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&Page) -> R,
    {
        let frames = &self.pool.frames[self.frame_idx];
        let guard = frames.read().unwrap();
        let frame = guard.as_ref().unwrap();
        f(&frame.page)
    }

    /// 페이지를 수정 가능하게 접근 (자동으로 dirty 표시)
    pub fn write<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut Page) -> R,
    {
        let frames = &self.pool.frames[self.frame_idx];
        let mut guard = frames.write().unwrap();
        let frame = guard.as_mut().unwrap();
        frame.is_dirty = true;
        f(&mut frame.page)
    }
}

impl<'a> Drop for PageGuard<'a> {
    fn drop(&mut self) {
        // pin_count 감소
        let frame_lock = &self.pool.frames[self.frame_idx];
        let mut guard = frame_lock.write().unwrap();
        if let Some(frame) = guard.as_mut() {
            frame.pin_count -= 1;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BufferPoolError {
    #[error("buffer pool is full: all frames are pinned")]
    NoFreeFrames,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl BufferPool {
    /// 새 버퍼 풀 생성
    ///
    /// capacity: 메모리에 보관할 최대 페이지 수
    pub fn new(capacity: usize, file_manager: FileManager) -> Self {
        let frames: Vec<RwLock<Option<BufferFrame>>> =
            (0..capacity).map(|_| RwLock::new(None)).collect();
        let free_list: Vec<usize> = (0..capacity).rev().collect();

        Self {
            capacity,
            page_table: Mutex::new(HashMap::new()),
            frames,
            lru: Mutex::new(LruList::new()),
            free_list: Mutex::new(free_list),
            file_manager: Mutex::new(file_manager),
        }
    }

    /// 페이지를 가져온다 (캐시 히트 또는 디스크 로드)
    ///
    /// 반환된 PageGuard가 살아있는 동안 해당 페이지는 교체되지 않는다.
    pub fn fetch_page(&self, page_id: PageId) -> Result<PageGuard<'_>, BufferPoolError> {
        // 1. 캐시 히트 확인
        {
            let page_table = self.page_table.lock().unwrap();
            if let Some(&frame_idx) = page_table.get(&page_id) {
                // pin_count 증가
                let mut guard = self.frames[frame_idx].write().unwrap();
                guard.as_mut().unwrap().pin_count += 1;

                // LRU에서 head로 이동 (최근 사용)
                let mut lru = self.lru.lock().unwrap();
                lru.touch(frame_idx);

                return Ok(PageGuard {
                    pool: self,
                    frame_idx,
                });
            }
        }

        // 2. 캐시 미스 — 빈 프레임 확보
        let frame_idx = self.get_free_frame()?;

        // 3. 디스크에서 페이지 읽기
        let page = {
            let mut fm = self.file_manager.lock().unwrap();
            fm.read_page(page_id.page_no)?
        };

        // 4. 프레임에 적재
        {
            let mut frame_guard = self.frames[frame_idx].write().unwrap();
            let mut frame = BufferFrame::new(page);
            frame.pin_count = 1;
            *frame_guard = Some(frame);
        }

        // 5. page_table과 LRU 갱신
        {
            let mut page_table = self.page_table.lock().unwrap();
            page_table.insert(page_id, frame_idx);

            let mut lru = self.lru.lock().unwrap();
            lru.touch(frame_idx);
        }

        Ok(PageGuard {
            pool: self,
            frame_idx,
        })
    }

    /// 새 페이지를 할당하고 버퍼 풀에 적재
    pub fn new_page(&self, page_type: PageType) -> Result<PageGuard<'_>, BufferPoolError> {
        let frame_idx = self.get_free_frame()?;

        // 디스크에 새 페이지 할당
        let page = {
            let mut fm = self.file_manager.lock().unwrap();
            fm.allocate_page(page_type)?
        };

        let page_id = page.page_id();

        // 프레임에 적재
        {
            let mut frame_guard = self.frames[frame_idx].write().unwrap();
            let mut frame = BufferFrame::new(page);
            frame.pin_count = 1;
            *frame_guard = Some(frame);
        }

        {
            let mut page_table = self.page_table.lock().unwrap();
            page_table.insert(page_id, frame_idx);

            let mut lru = self.lru.lock().unwrap();
            lru.touch(frame_idx);
        }

        Ok(PageGuard {
            pool: self,
            frame_idx,
        })
    }

    /// 특정 페이지를 디스크에 기록
    pub fn flush_page(&self, page_id: PageId) -> Result<(), BufferPoolError> {
        let frame_idx = {
            let page_table = self.page_table.lock().unwrap();
            match page_table.get(&page_id) {
                Some(&idx) => idx,
                None => return Ok(()), // 버퍼에 없으면 할 일 없음
            }
        };

        let mut frame_guard = self.frames[frame_idx].write().unwrap();
        if let Some(frame) = frame_guard.as_mut() {
            if frame.is_dirty {
                frame.page.update_checksum();
                let mut fm = self.file_manager.lock().unwrap();
                fm.write_page(&frame.page)?;
                frame.is_dirty = false;
            }
        }

        Ok(())
    }

    /// 모든 dirty 페이지를 디스크에 기록
    pub fn flush_all(&self) -> Result<(), BufferPoolError> {
        let page_ids: Vec<PageId> = {
            let page_table = self.page_table.lock().unwrap();
            page_table.keys().cloned().collect()
        };

        for page_id in page_ids {
            self.flush_page(page_id)?;
        }

        Ok(())
    }

    /// 빈 프레임 인덱스를 확보한다 (free list에서 꺼내거나 evict)
    fn get_free_frame(&self) -> Result<usize, BufferPoolError> {
        // 1. free list에서 먼저 확보 시도
        {
            let mut free_list = self.free_list.lock().unwrap();
            if let Some(frame_idx) = free_list.pop() {
                return Ok(frame_idx);
            }
        }

        // 2. free list가 비었으면 LRU에서 evict
        self.evict_one()
    }

    /// LRU tail에서 교체 가능한 프레임을 찾아 evict
    fn evict_one(&self) -> Result<usize, BufferPoolError> {
        let mut lru = self.lru.lock().unwrap();

        // pin_count == 0인 프레임을 찾을 때까지 시도
        let mut candidates = Vec::new();

        loop {
            match lru.evict() {
                Some(frame_idx) => {
                    let guard = self.frames[frame_idx].read().unwrap();
                    if let Some(frame) = guard.as_ref() {
                        if frame.pin_count == 0 {
                            // 교체 가능! 먼저 pinned 아닌 후보들을 다시 넣기
                            for c in candidates {
                                lru.touch(c);
                            }
                            drop(guard);
                            drop(lru);

                            // dirty이면 디스크에 먼저 기록
                            self.evict_frame(frame_idx)?;
                            return Ok(frame_idx);
                        } else {
                            // pinned — 나중에 다시 넣기 위해 보관
                            candidates.push(frame_idx);
                        }
                    }
                }
                None => {
                    // 모든 프레임이 pinned
                    for c in candidates {
                        lru.touch(c);
                    }
                    return Err(BufferPoolError::NoFreeFrames);
                }
            }
        }
    }

    /// 프레임을 실제로 evict: dirty면 flush, page_table에서 제거
    fn evict_frame(&self, frame_idx: usize) -> Result<(), BufferPoolError> {
        let mut frame_guard = self.frames[frame_idx].write().unwrap();
        if let Some(frame) = frame_guard.as_mut() {
            // dirty면 디스크에 기록
            if frame.is_dirty {
                frame.page.update_checksum();
                let mut fm = self.file_manager.lock().unwrap();
                fm.write_page(&frame.page)?;
            }

            // page_table에서 제거
            let page_id = frame.page.page_id();
            let mut page_table = self.page_table.lock().unwrap();
            page_table.remove(&page_id);
        }

        // 프레임 비우기
        *frame_guard = None;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_pool(capacity: usize) -> (BufferPool, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let fm = FileManager::open_or_create(0, tmp.path()).unwrap();
        let pool = BufferPool::new(capacity, fm);
        (pool, tmp)
    }

    fn setup_pool_with_pages(capacity: usize, num_pages: u32) -> (BufferPool, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        // 먼저 페이지들을 디스크에 생성
        {
            let mut fm = FileManager::open_or_create(0, tmp.path()).unwrap();
            for i in 0..num_pages {
                let page_id = PageId::new(0, i);
                let mut page = Page::new(page_id, PageType::Index);
                let msg = format!("page-{}", i);
                page.write_data(0, msg.as_bytes()).unwrap();
                page.update_checksum();
                fm.write_page(&page).unwrap();
            }
        }
        let fm = FileManager::open_or_create(0, tmp.path()).unwrap();
        let pool = BufferPool::new(capacity, fm);
        (pool, tmp)
    }

    #[test]
    fn test_fetch_page_from_disk() {
        let (pool, _tmp) = setup_pool_with_pages(4, 3);

        let guard = pool.fetch_page(PageId::new(0, 1)).unwrap();
        let data = guard.read(|page| {
            page.read_data(0, 6).unwrap().to_vec()
        });
        assert_eq!(&data, b"page-1");
    }

    #[test]
    fn test_cache_hit() {
        let (pool, _tmp) = setup_pool_with_pages(4, 3);

        // 첫 번째 fetch — 디스크에서 읽음
        let guard1 = pool.fetch_page(PageId::new(0, 0)).unwrap();
        drop(guard1);

        // 두 번째 fetch — 캐시 히트
        let guard2 = pool.fetch_page(PageId::new(0, 0)).unwrap();
        let data = guard2.read(|page| {
            page.read_data(0, 6).unwrap().to_vec()
        });
        assert_eq!(&data, b"page-0");
    }

    #[test]
    fn test_write_and_flush() {
        let (pool, tmp) = setup_pool_with_pages(4, 1);
        let page_id = PageId::new(0, 0);

        // 페이지 수정
        {
            let guard = pool.fetch_page(page_id).unwrap();
            guard.write(|page| {
                page.write_data(0, b"modified!").unwrap();
            });
        }

        // flush
        pool.flush_page(page_id).unwrap();

        // 새 FileManager로 디스크에서 직접 읽어서 확인
        let mut fm = FileManager::open_or_create(0, tmp.path()).unwrap();
        let page = fm.read_page(0).unwrap();
        let data = page.read_data(0, 9).unwrap();
        assert_eq!(data, b"modified!");
    }

    #[test]
    fn test_eviction() {
        // 버퍼 풀 크기 2, 페이지 3개 → evict 발생
        let (pool, _tmp) = setup_pool_with_pages(2, 3);

        let g0 = pool.fetch_page(PageId::new(0, 0)).unwrap();
        drop(g0);

        let g1 = pool.fetch_page(PageId::new(0, 1)).unwrap();
        drop(g1);

        // 2개 프레임이 꽉 찬 상태에서 3번째 페이지 요청 → evict 발생
        let g2 = pool.fetch_page(PageId::new(0, 2)).unwrap();
        let data = g2.read(|page| {
            page.read_data(0, 6).unwrap().to_vec()
        });
        assert_eq!(&data, b"page-2");
    }

    #[test]
    fn test_eviction_skips_pinned() {
        let (pool, _tmp) = setup_pool_with_pages(2, 3);

        // page 0을 pin한 채로 유지
        let _g0 = pool.fetch_page(PageId::new(0, 0)).unwrap();

        let g1 = pool.fetch_page(PageId::new(0, 1)).unwrap();
        drop(g1);

        // page 2 요청 → page 0은 pinned이라 evict 불가 → page 1이 evict됨
        let g2 = pool.fetch_page(PageId::new(0, 2)).unwrap();
        let data = g2.read(|page| {
            page.read_data(0, 6).unwrap().to_vec()
        });
        assert_eq!(&data, b"page-2");

        // page 0은 여전히 접근 가능 (evict 안 됨)
        let data = _g0.read(|page| {
            page.read_data(0, 6).unwrap().to_vec()
        });
        assert_eq!(&data, b"page-0");
    }

    #[test]
    fn test_all_pinned_returns_error() {
        let (pool, _tmp) = setup_pool_with_pages(2, 3);

        // 2개 프레임 모두 pin
        let _g0 = pool.fetch_page(PageId::new(0, 0)).unwrap();
        let _g1 = pool.fetch_page(PageId::new(0, 1)).unwrap();

        // 3번째 요청 → 모든 프레임이 pinned → 에러
        let result = pool.fetch_page(PageId::new(0, 2));
        assert!(result.is_err());
    }

    #[test]
    fn test_new_page() {
        let (pool, _tmp) = setup_pool(4);

        let guard = pool.new_page(PageType::Index).unwrap();
        let page_no = guard.read(|page| page.page_no());
        assert_eq!(page_no, 0);

        let guard2 = pool.new_page(PageType::Index).unwrap();
        let page_no2 = guard2.read(|page| page.page_no());
        assert_eq!(page_no2, 1);
    }

    #[test]
    fn test_dirty_eviction_persists() {
        let (pool, tmp) = setup_pool_with_pages(1, 2);

        // page 0을 수정
        {
            let guard = pool.fetch_page(PageId::new(0, 0)).unwrap();
            guard.write(|page| {
                page.write_data(0, b"dirty!").unwrap();
            });
        }

        // page 1 요청 → page 0 evict (dirty이므로 디스크에 기록됨)
        {
            let guard = pool.fetch_page(PageId::new(0, 1)).unwrap();
            drop(guard);
        }

        // 디스크에서 직접 확인
        let mut fm = FileManager::open_or_create(0, tmp.path()).unwrap();
        let page = fm.read_page(0).unwrap();
        let data = page.read_data(0, 6).unwrap();
        assert_eq!(data, b"dirty!");
    }
}
