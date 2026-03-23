use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::page::{Page, PAGE_SIZE};
use super::PageId;
use super::PageType;

/// 테이블스페이스 파일 관리자
///
/// 하나의 FileManager는 하나의 테이블스페이스 파일(.ibd)을 관리한다.
/// 페이지 번호를 기반으로 파일 내 오프셋을 계산하여 읽기/쓰기를 수행한다.
pub struct FileManager {
    space_id: u32,
    file: File,
}

impl FileManager {
    /// 기존 파일을 열거나 새 파일을 생성
    pub fn open_or_create(space_id: u32, path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        Ok(Self { space_id, file })
    }

    /// 특정 페이지를 디스크에서 읽기
    pub fn read_page(&mut self, page_no: u32) -> io::Result<Page> {
        let offset = page_no as u64 * PAGE_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;

        let mut buf = [0u8; PAGE_SIZE];
        self.file.read_exact(&mut buf)?;

        Ok(Page::from_bytes(buf))
    }

    /// 페이지를 디스크에 쓰기
    pub fn write_page(&mut self, page: &Page) -> io::Result<()> {
        let offset = page.page_no() as u64 * PAGE_SIZE as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(page.as_bytes())?;
        Ok(())
    }

    /// 페이지를 디스크에 쓰고 fsync로 영속성 보장
    pub fn write_page_durable(&mut self, page: &Page) -> io::Result<()> {
        self.write_page(page)?;
        self.file.sync_all()
    }

    /// 새 페이지를 할당 (파일 끝에 빈 페이지 추가)
    pub fn allocate_page(&mut self, page_type: PageType) -> io::Result<Page> {
        let file_len = self.file.metadata()?.len();
        let page_no = (file_len / PAGE_SIZE as u64) as u32;

        let page_id = PageId::new(self.space_id, page_no);
        let mut page = Page::new(page_id, page_type);
        page.update_checksum();

        self.write_page(&page)?;
        Ok(page)
    }

    /// 파일에 저장된 총 페이지 수
    pub fn page_count(&self) -> io::Result<u32> {
        let len = self.file.metadata()?.len();
        Ok((len / PAGE_SIZE as u64) as u32)
    }

    pub fn space_id(&self) -> u32 {
        self.space_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_write_and_read_page() {
        let tmp = NamedTempFile::new().unwrap();
        let mut fm = FileManager::open_or_create(0, tmp.path()).unwrap();

        // 페이지 생성 및 데이터 쓰기
        let page_id = PageId::new(0, 0);
        let mut page = Page::new(page_id, PageType::Index);
        page.write_data(0, b"hello from disk").unwrap();
        page.update_checksum();

        // 디스크에 쓰기
        fm.write_page(&page).unwrap();

        // 디스크에서 읽기
        let read_page = fm.read_page(0).unwrap();
        assert_eq!(read_page.page_no(), 0);
        assert_eq!(read_page.space_id(), 0);
        assert!(read_page.verify_checksum());

        let data = read_page.read_data(0, 15).unwrap();
        assert_eq!(data, b"hello from disk");
    }

    #[test]
    fn test_multiple_pages() {
        let tmp = NamedTempFile::new().unwrap();
        let mut fm = FileManager::open_or_create(1, tmp.path()).unwrap();

        // 3개의 페이지를 순차적으로 쓰기
        for i in 0..3u32 {
            let page_id = PageId::new(1, i);
            let mut page = Page::new(page_id, PageType::Index);
            let msg = format!("page-{}", i);
            page.write_data(0, msg.as_bytes()).unwrap();
            page.update_checksum();
            fm.write_page(&page).unwrap();
        }

        assert_eq!(fm.page_count().unwrap(), 3);

        // 역순으로 읽어서 검증
        for i in (0..3u32).rev() {
            let page = fm.read_page(i).unwrap();
            assert_eq!(page.page_no(), i);
            assert!(page.verify_checksum());

            let expected = format!("page-{}", i);
            let data = page.read_data(0, expected.len()).unwrap();
            assert_eq!(data, expected.as_bytes());
        }
    }

    #[test]
    fn test_allocate_page() {
        let tmp = NamedTempFile::new().unwrap();
        let mut fm = FileManager::open_or_create(0, tmp.path()).unwrap();

        let p0 = fm.allocate_page(PageType::Index).unwrap();
        assert_eq!(p0.page_no(), 0);

        let p1 = fm.allocate_page(PageType::UndoLog).unwrap();
        assert_eq!(p1.page_no(), 1);

        assert_eq!(fm.page_count().unwrap(), 2);
    }
}
