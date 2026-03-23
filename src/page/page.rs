use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

use super::{PageId, PageType};

/// InnoDB 페이지 크기: 16KB
pub const PAGE_SIZE: usize = 16 * 1024;

/// FIL Header 오프셋 (페이지 시작부터)
/// InnoDB의 모든 페이지는 38바이트의 FIL Header로 시작한다.
mod fil_header {
    /// 체크섬 (4 bytes)
    pub const CHECKSUM: usize = 0;
    /// 페이지 번호 (4 bytes)
    pub const PAGE_NO: usize = 4;
    /// B+Tree 리프의 이전 페이지 (4 bytes)
    pub const PREV: usize = 8;
    /// B+Tree 리프의 다음 페이지 (4 bytes)
    pub const NEXT: usize = 12;
    /// 마지막 수정 LSN (8 bytes)
    pub const LSN: usize = 16;
    /// 페이지 타입 (2 bytes)
    pub const PAGE_TYPE: usize = 24;
    /// flush된 LSN (8 bytes) — space 0의 page 0에서만 의미 있음
    pub const FLUSH_LSN: usize = 26;
    /// Space ID (4 bytes)
    pub const SPACE_ID: usize = 34;
    /// FIL Header 전체 크기
    pub const SIZE: usize = 38;
}

/// FIL Trailer 오프셋 (페이지 끝 8바이트)
mod fil_trailer {
    use super::PAGE_SIZE;
    /// 체크섬 (4 bytes)
    pub const CHECKSUM: usize = PAGE_SIZE - 8;
    /// LSN 하위 4바이트 (4 bytes)
    pub const LSN_LOW: usize = PAGE_SIZE - 4;
}

/// 16KB 고정 크기 디스크 페이지
pub struct Page {
    data: Box<[u8; PAGE_SIZE]>,
}

impl Page {
    /// 빈 페이지 생성
    pub fn new(page_id: PageId, page_type: PageType) -> Self {
        let mut page = Self {
            data: Box::new([0u8; PAGE_SIZE]),
        };
        page.set_page_no(page_id.page_no);
        page.set_space_id(page_id.space_id);
        page.set_page_type(page_type);
        page
    }

    /// 바이트 배열로부터 페이지 생성
    pub fn from_bytes(bytes: [u8; PAGE_SIZE]) -> Self {
        Self {
            data: Box::new(bytes),
        }
    }

    /// 페이지의 원시 바이트 참조
    pub fn as_bytes(&self) -> &[u8; PAGE_SIZE] {
        &self.data
    }

    /// 페이지의 원시 바이트 가변 참조
    pub fn as_bytes_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
        &mut self.data
    }

    // ── FIL Header 읽기 ──

    pub fn checksum(&self) -> u32 {
        let mut cursor = Cursor::new(&self.data[fil_header::CHECKSUM..]);
        cursor.read_u32::<BigEndian>().unwrap()
    }

    pub fn page_no(&self) -> u32 {
        let mut cursor = Cursor::new(&self.data[fil_header::PAGE_NO..]);
        cursor.read_u32::<BigEndian>().unwrap()
    }

    pub fn prev_page(&self) -> u32 {
        let mut cursor = Cursor::new(&self.data[fil_header::PREV..]);
        cursor.read_u32::<BigEndian>().unwrap()
    }

    pub fn next_page(&self) -> u32 {
        let mut cursor = Cursor::new(&self.data[fil_header::NEXT..]);
        cursor.read_u32::<BigEndian>().unwrap()
    }

    pub fn lsn(&self) -> u64 {
        let mut cursor = Cursor::new(&self.data[fil_header::LSN..]);
        cursor.read_u64::<BigEndian>().unwrap()
    }

    pub fn page_type(&self) -> Option<PageType> {
        let mut cursor = Cursor::new(&self.data[fil_header::PAGE_TYPE..]);
        let raw = cursor.read_u16::<BigEndian>().unwrap();
        PageType::from_u16(raw)
    }

    pub fn space_id(&self) -> u32 {
        let mut cursor = Cursor::new(&self.data[fil_header::SPACE_ID..]);
        cursor.read_u32::<BigEndian>().unwrap()
    }

    pub fn page_id(&self) -> PageId {
        PageId::new(self.space_id(), self.page_no())
    }

    // ── FIL Header 쓰기 ──

    pub fn set_page_no(&mut self, page_no: u32) {
        let mut cursor = Cursor::new(&mut self.data[fil_header::PAGE_NO..]);
        cursor.write_u32::<BigEndian>(page_no).unwrap();
    }

    pub fn set_prev_page(&mut self, prev: u32) {
        let mut cursor = Cursor::new(&mut self.data[fil_header::PREV..]);
        cursor.write_u32::<BigEndian>(prev).unwrap();
    }

    pub fn set_next_page(&mut self, next: u32) {
        let mut cursor = Cursor::new(&mut self.data[fil_header::NEXT..]);
        cursor.write_u32::<BigEndian>(next).unwrap();
    }

    pub fn set_lsn(&mut self, lsn: u64) {
        // FIL Header LSN
        let mut cursor = Cursor::new(&mut self.data[fil_header::LSN..]);
        cursor.write_u64::<BigEndian>(lsn).unwrap();
        // FIL Trailer LSN (하위 4바이트)
        let mut cursor = Cursor::new(&mut self.data[fil_trailer::LSN_LOW..]);
        cursor.write_u32::<BigEndian>(lsn as u32).unwrap();
    }

    pub fn set_page_type(&mut self, page_type: PageType) {
        let mut cursor = Cursor::new(&mut self.data[fil_header::PAGE_TYPE..]);
        cursor.write_u16::<BigEndian>(page_type.as_u16()).unwrap();
    }

    pub fn set_space_id(&mut self, space_id: u32) {
        let mut cursor = Cursor::new(&mut self.data[fil_header::SPACE_ID..]);
        cursor.write_u32::<BigEndian>(space_id).unwrap();
    }

    // ── 체크섬 ──

    /// CRC32 체크섬 계산 (헤더/트레일러의 체크섬 필드 자체는 제외)
    pub fn compute_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        // 헤더 체크섬(처음 4바이트)과 트레일러 체크섬(끝에서 8~4바이트)을 제외
        hasher.update(&self.data[fil_header::PAGE_NO..fil_trailer::CHECKSUM]);
        hasher.update(&self.data[fil_trailer::LSN_LOW..]);
        hasher.finalize()
    }

    /// 체크섬을 계산하여 헤더와 트레일러에 기록
    pub fn update_checksum(&mut self) {
        let checksum = self.compute_checksum();
        // FIL Header checksum
        let mut cursor = Cursor::new(&mut self.data[fil_header::CHECKSUM..]);
        cursor.write_u32::<BigEndian>(checksum).unwrap();
        // FIL Trailer checksum
        let mut cursor = Cursor::new(&mut self.data[fil_trailer::CHECKSUM..]);
        cursor.write_u32::<BigEndian>(checksum).unwrap();
    }

    /// 체크섬 검증
    pub fn verify_checksum(&self) -> bool {
        let stored = self.checksum();
        let computed = self.compute_checksum();
        stored == computed
    }

    // ── 사용자 데이터 영역 ──

    /// 사용자 데이터를 쓸 수 있는 영역의 시작 오프셋
    pub const DATA_START: usize = fil_header::SIZE;
    /// 사용자 데이터를 쓸 수 있는 영역의 끝 오프셋 (exclusive)
    pub const DATA_END: usize = PAGE_SIZE - 8; // FIL Trailer 앞까지

    /// 사용자 데이터 영역에 바이트 쓰기
    pub fn write_data(&mut self, offset: usize, data: &[u8]) -> Result<(), PageError> {
        let abs_offset = Self::DATA_START + offset;
        if abs_offset + data.len() > Self::DATA_END {
            return Err(PageError::OutOfBounds {
                offset: abs_offset,
                len: data.len(),
            });
        }
        self.data[abs_offset..abs_offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// 사용자 데이터 영역에서 바이트 읽기
    pub fn read_data(&self, offset: usize, len: usize) -> Result<&[u8], PageError> {
        let abs_offset = Self::DATA_START + offset;
        if abs_offset + len > Self::DATA_END {
            return Err(PageError::OutOfBounds {
                offset: abs_offset,
                len,
            });
        }
        Ok(&self.data[abs_offset..abs_offset + len])
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PageError {
    #[error("data access out of bounds: offset={offset}, len={len}")]
    OutOfBounds { offset: usize, len: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_page_has_correct_id() {
        let id = PageId::new(1, 42);
        let page = Page::new(id, PageType::Index);

        assert_eq!(page.page_no(), 42);
        assert_eq!(page.space_id(), 1);
        assert_eq!(page.page_id(), id);
        assert_eq!(page.page_type(), Some(PageType::Index));
    }

    #[test]
    fn test_checksum_roundtrip() {
        let mut page = Page::new(PageId::new(0, 0), PageType::Index);
        page.write_data(0, b"hello innodb").unwrap();
        page.update_checksum();

        assert!(page.verify_checksum());
    }

    #[test]
    fn test_checksum_detects_corruption() {
        let mut page = Page::new(PageId::new(0, 0), PageType::Index);
        page.write_data(0, b"hello innodb").unwrap();
        page.update_checksum();

        // 데이터 변조
        page.as_bytes_mut()[100] ^= 0xFF;
        assert!(!page.verify_checksum());
    }

    #[test]
    fn test_lsn_roundtrip() {
        let mut page = Page::new(PageId::new(0, 0), PageType::Index);
        page.set_lsn(123456789);
        assert_eq!(page.lsn(), 123456789);
    }

    #[test]
    fn test_prev_next_page() {
        let mut page = Page::new(PageId::new(0, 5), PageType::Index);
        page.set_prev_page(4);
        page.set_next_page(6);
        assert_eq!(page.prev_page(), 4);
        assert_eq!(page.next_page(), 6);
    }

    #[test]
    fn test_data_write_read() {
        let mut page = Page::new(PageId::new(0, 0), PageType::Index);
        let data = b"test data for innodb page";
        page.write_data(0, data).unwrap();

        let read = page.read_data(0, data.len()).unwrap();
        assert_eq!(read, data);
    }

    #[test]
    fn test_data_out_of_bounds() {
        let mut page = Page::new(PageId::new(0, 0), PageType::Index);
        let huge = vec![0u8; PAGE_SIZE]; // 페이지 전체 크기만큼 — 데이터 영역 초과
        assert!(page.write_data(0, &huge).is_err());
    }
}
