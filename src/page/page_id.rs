/// 페이지 식별자
///
/// InnoDB에서 모든 페이지는 (space_id, page_no) 튜플로 고유하게 식별된다.
/// - space_id: 테이블스페이스 ID (하나의 .ibd 파일에 대응)
/// - page_no: 해당 테이블스페이스 내에서의 페이지 번호 (0부터 시작)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageId {
    pub space_id: u32,
    pub page_no: u32,
}

impl PageId {
    pub fn new(space_id: u32, page_no: u32) -> Self {
        Self { space_id, page_no }
    }
}

impl std::fmt::Display for PageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.space_id, self.page_no)
    }
}
