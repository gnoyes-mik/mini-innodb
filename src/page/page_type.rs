/// InnoDB 페이지 타입
///
/// 각 페이지는 용도에 따라 타입이 지정된다.
/// 헤더의 FIL_PAGE_TYPE 필드 (2 bytes)에 저장된다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum PageType {
    /// 새로 할당되어 아직 초기화되지 않은 페이지
    Allocated = 0,
    /// Undo log 페이지
    UndoLog = 2,
    /// 파일 세그먼트 inode
    Inode = 3,
    /// Insert buffer free list
    IbufFreeList = 4,
    /// Insert buffer bitmap
    IbufBitmap = 5,
    /// 시스템 내부 페이지
    System = 6,
    /// 트랜잭션 시스템 헤더
    TrxSystem = 7,
    /// 테이블스페이스 헤더 (FSP)
    FspHeader = 8,
    /// Extent descriptor 페이지
    ExtentDescriptor = 9,
    /// BLOB 페이지
    Blob = 10,
    /// B+Tree 노드 (인덱스 페이지)
    Index = 17855,
}

impl PageType {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(Self::Allocated),
            2 => Some(Self::UndoLog),
            3 => Some(Self::Inode),
            4 => Some(Self::IbufFreeList),
            5 => Some(Self::IbufBitmap),
            6 => Some(Self::System),
            7 => Some(Self::TrxSystem),
            8 => Some(Self::FspHeader),
            9 => Some(Self::ExtentDescriptor),
            10 => Some(Self::Blob),
            17855 => Some(Self::Index),
            _ => None,
        }
    }

    pub fn as_u16(self) -> u16 {
        self as u16
    }
}
