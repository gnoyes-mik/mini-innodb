# InnoDB Core Re-implementation in Rust

## Overview

InnoDB 스토리지 엔진의 핵심 기능을 Rust로 재구현하는 학습 프로젝트.
원본 InnoDB의 설계 철학을 이해하고, Rust의 안전성과 성능을 활용하여 각 모듈을 단계별로 구현한다.

**목표**: 완전한 스토리지 엔진이 아닌, InnoDB의 핵심 메커니즘을 이해하고 동작하는 프로토타입을 만드는 것

**프로젝트 이름**: `mini-innodb`

---

## 프로젝트 구조

```
mini-innodb/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   ├── page/           # Phase 1: 디스크 페이지 관리
│   ├── buffer/         # Phase 2: 버퍼 풀
│   ├── btree/          # Phase 3: B+Tree 인덱스
│   ├── record/         # Phase 4: 레코드 포맷
│   ├── log/            # Phase 5: WAL (Write-Ahead Logging)
│   ├── tx/             # Phase 6: 트랜잭션 & MVCC
│   ├── lock/           # Phase 7: Lock 매니저
│   └── sql/            # Phase 8: 간단한 SQL 인터페이스
├── tests/
│   ├── page_tests.rs
│   ├── buffer_tests.rs
│   ├── btree_tests.rs
│   ├── record_tests.rs
│   ├── log_tests.rs
│   ├── tx_tests.rs
│   ├── lock_tests.rs
│   └── integration_tests.rs
└── data/               # 런타임 데이터 파일
```

---

## Phase 1: 디스크 페이지 관리 (Page Management)

### 학습 목표
- InnoDB가 데이터를 디스크에 저장하는 최소 단위(Page)를 이해한다
- 고정 크기 페이지의 읽기/쓰기를 구현한다

### InnoDB 원본 참고
- `storage/innobase/fil/` — 파일 관리
- `storage/innobase/page/` — 페이지 구조
- 기본 페이지 크기: 16KB (`FIL_PAGE_SIZE`)

### 구현 항목

| 항목 | 설명 |
|------|------|
| `PageId` | `(space_id, page_no)` 튜플로 페이지 식별 |
| `Page` | 16KB 고정 크기 바이트 배열 |
| `PageHeader` | 체크섬, 페이지 번호, 페이지 타입, LSN 등 |
| `PageType` | `FIL_PAGE_INDEX`, `FIL_PAGE_UNDO_LOG`, `FIL_PAGE_INODE` 등 |
| `FileManager` | 파일 열기/닫기, 페이지 단위 읽기/쓰기 |

### Rust 구현 가이드

```rust
/// 페이지 크기 상수
const PAGE_SIZE: usize = 16 * 1024; // 16KB

/// 페이지 식별자
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PageId {
    space_id: u32,
    page_no: u32,
}

/// 디스크 페이지
struct Page {
    id: PageId,
    data: [u8; PAGE_SIZE],
}

/// 페이지 헤더 (Page 시작 38바이트)
#[repr(C)]
struct PageHeader {
    checksum: u32,        // 4 bytes
    page_no: u32,         // 4 bytes
    prev_page: u32,       // 4 bytes (B+Tree leaf 연결)
    next_page: u32,       // 4 bytes (B+Tree leaf 연결)
    lsn: u64,             // 8 bytes (마지막 수정 LSN)
    page_type: u16,       // 2 bytes
    // ...
}

/// 파일 I/O — std::fs::File + seek 기반
struct FileManager {
    file: std::fs::File,
}

impl FileManager {
    fn read_page(&self, page_no: u32) -> io::Result<Page> { /* ... */ }
    fn write_page(&self, page: &Page) -> io::Result<()> { /* ... */ }
}
```

### Rust 팁
- `[u8; PAGE_SIZE]`는 스택에 16KB를 잡으므로, `Box<[u8; PAGE_SIZE]>` 또는 `Vec<u8>` 사용 고려
- 바이트 직렬화에 `byteorder` 크레이트 활용 (Big Endian — InnoDB 기본)
- 체크섬은 `crc32` 크레이트로 구현

### 검증
- [ ] 페이지를 파일에 쓰고 다시 읽었을 때 동일한 데이터 확인
- [ ] 체크섬 검증 성공/실패 테스트
- [ ] 여러 페이지를 순차적으로 읽기/쓰기

---

## Phase 2: 버퍼 풀 (Buffer Pool)

### 학습 목표
- 디스크 I/O를 최소화하기 위한 페이지 캐싱 메커니즘을 이해한다
- LRU 알고리즘 기반 페이지 교체 정책을 구현한다

### InnoDB 원본 참고
- `storage/innobase/buf/` — 버퍼 풀 전체
- `buf0buf.cc` — 버퍼 풀 핵심 로직
- `buf0lru.cc` — LRU 리스트 관리
- InnoDB는 young/old 영역을 나눈 변형 LRU 사용

### 구현 항목

| 항목 | 설명 |
|------|------|
| `BufferPool` | 고정 크기 페이지 프레임 배열 관리 |
| `BufferFrame` | 페이지 데이터 + 메타데이터 (dirty flag, pin count, etc.) |
| `LruList` | 페이지 교체를 위한 LRU 리스트 |
| `PageTable` | `PageId → BufferFrame` 매핑 (해시맵) |
| `flush` | dirty 페이지를 디스크에 기록 |

### Rust 구현 가이드

```rust
use std::collections::{HashMap, LinkedList};
use std::sync::{Arc, RwLock, Mutex};

struct BufferFrame {
    page: Page,
    is_dirty: bool,
    pin_count: u32,
}

struct BufferPool {
    capacity: usize,
    page_table: HashMap<PageId, usize>,  // PageId → frame index
    frames: Vec<RwLock<BufferFrame>>,
    lru: Mutex<LinkedList<usize>>,       // frame indices
    file_manager: FileManager,
}

impl BufferPool {
    /// 페이지를 버퍼에서 가져오거나 디스크에서 로드
    fn fetch_page(&self, page_id: PageId) -> Result<PageGuard> { /* ... */ }

    /// dirty 페이지를 디스크에 기록
    fn flush_page(&self, page_id: PageId) -> Result<()> { /* ... */ }

    /// LRU에서 교체할 프레임 선택 (pin_count == 0인 것)
    fn evict(&self) -> Result<usize> { /* ... */ }
}
```

### Rust 팁
- `RwLock<BufferFrame>`으로 읽기 동시성 확보, 쓰기 시 exclusive lock
- RAII 패턴으로 `PageGuard` 구현 — drop 시 자동으로 pin_count 감소 및 LRU 갱신
- `LinkedList` 대신 intrusive linked list를 구현하면 O(1) 제거 가능 (고급)

### 검증
- [ ] 캐시 히트 시 디스크 I/O가 발생하지 않는지 확인
- [ ] 버퍼 풀 용량 초과 시 LRU 교체 동작 확인
- [ ] dirty 페이지 flush 후 재시작 시 데이터 유지 확인
- [ ] 동시 접근 시 데이터 정합성 테스트

---

## Phase 3: B+Tree 인덱스

### 학습 목표
- InnoDB의 Clustered Index 구조를 이해한다
- 디스크 기반 B+Tree의 검색, 삽입, 페이지 분할을 구현한다

### InnoDB 원본 참고
- `storage/innobase/btr/` — B-Tree 연산
- `btr0btr.cc` — 트리 구조 관리
- `btr0cur.cc` — 커서 기반 탐색
- `btr0pcur.cc` — persistent cursor

### 구현 항목

| 항목 | 설명 |
|------|------|
| `BTreeIndex` | B+Tree 전체 관리 (root page 추적) |
| `InternalNode` | 내부 노드: `[key, child_page_no]` 쌍 |
| `LeafNode` | 리프 노드: `[key, record]` 쌍 + prev/next 포인터 |
| `Cursor` | 트리 탐색 위치를 추적하는 커서 |
| `search` | root → leaf 탐색 |
| `insert` | 삽입 + 필요 시 페이지 분할 (split) |
| `range_scan` | 리프 노드 연결 리스트를 따라 범위 스캔 |

### Rust 구현 가이드

```rust
/// B+Tree 노드 (페이지 내부 구조)
enum BTreeNode {
    Internal(InternalNode),
    Leaf(LeafNode),
}

struct InternalNode {
    keys: Vec<Key>,
    children: Vec<PageId>,  // keys.len() + 1 개
}

struct LeafNode {
    keys: Vec<Key>,
    values: Vec<Record>,
    prev_leaf: Option<PageId>,
    next_leaf: Option<PageId>,
}

struct BTreeIndex {
    root_page_id: PageId,
    buffer_pool: Arc<BufferPool>,
}

impl BTreeIndex {
    fn search(&self, key: &Key) -> Result<Option<Record>> { /* ... */ }
    fn insert(&self, key: Key, record: Record) -> Result<()> { /* ... */ }
    fn range_scan(&self, start: &Key, end: &Key) -> Result<Vec<Record>> { /* ... */ }

    /// 페이지가 가득 차면 분할
    fn split_leaf(&self, page_id: PageId) -> Result<(Key, PageId)> { /* ... */ }
    fn split_internal(&self, page_id: PageId) -> Result<(Key, PageId)> { /* ... */ }
}
```

### Rust 팁
- Key 비교에 `Ord` trait 구현
- 노드를 페이지 바이트 배열로 직렬화/역직렬화하는 코드가 핵심 — `TryFrom<&Page>` 활용
- 재귀 대신 반복문으로 탐색하면 스택 오버플로우 방지 + 성능 향상
- 삭제(delete)와 병합(merge)은 고급 과제로 분리 가능

### 검증
- [ ] 순차 삽입 후 정확한 검색 결과
- [ ] 랜덤 키 대량 삽입 후 range scan 정렬 확인
- [ ] 페이지 분할 후 트리 구조 무결성 확인
- [ ] 재시작 후에도 인덱스 데이터 유지

---

## Phase 4: 레코드 포맷 (Record Format)

### 학습 목표
- InnoDB의 행(row) 저장 형식을 이해한다
- COMPACT row format을 구현한다

### InnoDB 원본 참고
- `storage/innobase/rem/` — 레코드 관리
- `rem0rec.cc` — 레코드 접근 함수
- InnoDB row format: REDUNDANT, COMPACT, DYNAMIC, COMPRESSED

### 구현 항목

| 항목 | 설명 |
|------|------|
| `ColumnType` | INTEGER, VARCHAR, BLOB 등 데이터 타입 |
| `TableSchema` | 테이블 정의 (컬럼 목록, PK 정보) |
| `Record` | 레코드 직렬화/역직렬화 |
| `RecordHeader` | null bitmap, variable-length 필드 오프셋 |
| `RowId` | 숨겨진 PK (사용자 PK 없을 때) |

### Rust 구현 가이드

```rust
/// 지원할 컬럼 타입
#[derive(Debug, Clone)]
enum ColumnType {
    TinyInt,
    Int,
    BigInt,
    Varchar(usize),   // max length
    Blob,
}

/// 실제 값
#[derive(Debug, Clone)]
enum Value {
    Null,
    Int(i64),
    Bytes(Vec<u8>),
}

/// 테이블 스키마
struct TableSchema {
    name: String,
    columns: Vec<(String, ColumnType, bool)>,  // (name, type, nullable)
    primary_key: Vec<usize>,                    // column indices
}

/// COMPACT 레코드 포맷
struct RecordEncoder;

impl RecordEncoder {
    /// Record → bytes (COMPACT format)
    fn encode(schema: &TableSchema, values: &[Value]) -> Vec<u8> {
        // 1. Variable-length 필드 오프셋 리스트 (역순)
        // 2. Null bitmap
        // 3. Record header (5 bytes)
        // 4. 실제 컬럼 데이터
        todo!()
    }

    /// bytes → Record
    fn decode(schema: &TableSchema, data: &[u8]) -> Vec<Value> {
        todo!()
    }
}
```

### Rust 팁
- `enum Value`로 동적 타입 표현 — 패턴 매칭으로 타입 안전한 접근
- 가변 길이 필드 인코딩 시 `byteorder` 크레이트 활용
- `serde`는 사용하지 않기 — InnoDB 바이너리 포맷을 직접 구현하는 것이 학습 포인트

### 검증
- [ ] 다양한 타입 조합의 레코드 encode → decode 라운드트립
- [ ] NULL 값이 포함된 레코드 처리
- [ ] VARCHAR 가변 길이 필드 정확한 오프셋 계산

---

## Phase 5: WAL — Write-Ahead Logging

### 학습 목표
- Crash recovery의 핵심인 WAL 원리를 이해한다
- Redo log를 구현하여 비정상 종료 후 데이터 복구를 수행한다

### InnoDB 원본 참고
- `storage/innobase/log/` — 로그 시스템
- `log0log.cc` — 로그 버퍼 & flush
- `log0recv.cc` — crash recovery (redo 적용)
- 핵심 원칙: **WAL protocol** — 데이터 페이지를 디스크에 쓰기 전에 반드시 redo log를 먼저 디스크에 기록

### 구현 항목

| 항목 | 설명 |
|------|------|
| `LSN` | Log Sequence Number — 로그 내 위치 식별자 |
| `LogRecord` | 개별 redo 로그 레코드 |
| `LogBuffer` | 메모리 내 로그 버퍼 |
| `LogWriter` | 로그 파일에 순차 기록 |
| `Recovery` | 시작 시 redo log를 읽어 crash recovery 수행 |
| `Checkpoint` | 어디까지 flush 되었는지 기록 |

### Rust 구현 가이드

```rust
/// Log Sequence Number
type LSN = u64;

/// Redo 로그 레코드 타입
#[derive(Debug)]
enum LogRecordType {
    /// 페이지의 특정 오프셋에 바이트 쓰기
    PageWrite {
        page_id: PageId,
        offset: u16,
        data: Vec<u8>,
    },
    /// 트랜잭션 커밋
    Commit { tx_id: u64 },
    /// 체크포인트
    Checkpoint { flush_lsn: LSN },
}

struct LogRecord {
    lsn: LSN,
    record_type: LogRecordType,
}

struct LogManager {
    buffer: Mutex<Vec<u8>>,
    current_lsn: AtomicU64,
    flushed_lsn: AtomicU64,
    log_file: Mutex<File>,
}

impl LogManager {
    /// redo 로그 기록 — LSN 반환
    fn append(&self, record: LogRecordType) -> Result<LSN> { /* ... */ }

    /// 로그 버퍼를 디스크에 flush
    fn flush(&self, up_to_lsn: LSN) -> Result<()> { /* ... */ }
}

struct Recovery {
    log_manager: Arc<LogManager>,
    buffer_pool: Arc<BufferPool>,
}

impl Recovery {
    /// crash recovery: checkpoint 이후의 redo log를 재적용
    fn recover(&self) -> Result<()> {
        // 1. 마지막 checkpoint 찾기
        // 2. checkpoint LSN 이후의 로그 레코드 순차 읽기
        // 3. 각 페이지의 LSN과 비교하여 필요한 것만 redo 적용
        todo!()
    }
}
```

### Rust 팁
- `AtomicU64`로 LSN 관리 — lock-free 증가
- `fsync` (Unix: `File::sync_all()`)로 durability 보장
- 로그 파일은 순차 쓰기만 하므로 성능이 좋음 — 이것이 WAL의 핵심 이점
- 테스트 시 `tempfile` 크레이트로 임시 파일 활용

### 검증
- [ ] 로그 기록 후 flush, 재시작 시 로그 읽기 성공
- [ ] 의도적 crash (프로세스 kill) 후 recovery로 데이터 복구
- [ ] checkpoint 이후의 로그만 replay 되는지 확인
- [ ] WAL protocol: 페이지 flush 전 해당 LSN까지 로그 flush 확인

---

## Phase 6: 트랜잭션 & MVCC

### 학습 목표
- InnoDB의 MVCC (Multi-Version Concurrency Control) 메커니즘을 이해한다
- Undo log를 활용한 snapshot 읽기를 구현한다

### InnoDB 원본 참고
- `storage/innobase/trx/` — 트랜잭션 시스템
- `trx0trx.cc` — 트랜잭션 관리
- `trx0undo.cc` — undo 로그
- `read0read.cc` — Read View (snapshot)
- 격리 수준: InnoDB 기본은 REPEATABLE READ

### 구현 항목

| 항목 | 설명 |
|------|------|
| `Transaction` | 트랜잭션 상태 관리 (ACTIVE, COMMITTED, ABORTED) |
| `TxManager` | 트랜잭션 ID 할당, 활성 트랜잭션 목록 관리 |
| `UndoLog` | 변경 전 데이터를 저장하여 rollback & MVCC 지원 |
| `ReadView` | 트랜잭션 시작 시점의 가시성 판단 |
| `VersionChain` | 레코드의 버전 체인 (최신 → undo를 따라 과거 버전) |

### Rust 구현 가이드

```rust
type TxId = u64;

#[derive(Debug, PartialEq)]
enum TxState {
    Active,
    Committed,
    Aborted,
}

struct Transaction {
    id: TxId,
    state: TxState,
    undo_log: Vec<UndoRecord>,
    read_view: Option<ReadView>,
}

/// Undo 레코드: 변경 전 값 저장
struct UndoRecord {
    table_id: u32,
    primary_key: Key,
    old_values: Vec<Value>,      // 변경 전 컬럼 값
    prev_tx_id: TxId,            // 이전 버전의 트랜잭션 ID
}

/// Read View — MVCC 가시성 판단의 핵심
struct ReadView {
    /// 이 Read View 생성 시점에 활성 상태였던 트랜잭션 ID 목록
    active_tx_ids: Vec<TxId>,
    /// 가장 작은 활성 트랜잭션 ID
    min_active_tx_id: TxId,
    /// Read View 생성 시점의 다음 트랜잭션 ID
    max_tx_id: TxId,
    /// Read View를 만든 트랜잭션 ID
    creator_tx_id: TxId,
}

impl ReadView {
    /// 해당 트랜잭션이 만든 변경이 이 Read View에서 보이는가?
    fn is_visible(&self, data_tx_id: TxId) -> bool {
        if data_tx_id == self.creator_tx_id {
            return true;  // 자기 자신의 변경은 항상 보임
        }
        if data_tx_id < self.min_active_tx_id {
            return true;  // Read View 생성 전에 커밋된 트랜잭션
        }
        if data_tx_id >= self.max_tx_id {
            return false; // Read View 생성 후에 시작된 트랜잭션
        }
        // 그 사이: active 목록에 없으면 이미 커밋된 것 → 보임
        !self.active_tx_ids.contains(&data_tx_id)
    }
}
```

### Rust 팁
- `ReadView`의 `is_visible`이 MVCC의 핵심 — 이 함수 하나로 격리 수준이 결정됨
- REPEATABLE READ: 트랜잭션의 첫 SELECT 시 ReadView 생성, 이후 재사용
- READ COMMITTED: 매 SELECT마다 새 ReadView 생성
- Undo log의 버전 체인은 linked list처럼 동작 — `prev_tx_id`를 따라가며 보이는 버전을 찾음

### 검증
- [ ] 두 트랜잭션이 동시에 같은 행을 읽을 때 각자의 snapshot 확인
- [ ] 커밋되지 않은 변경은 다른 트랜잭션에서 보이지 않는지 확인
- [ ] ROLLBACK 시 undo log를 통해 원래 값 복원
- [ ] 버전 체인을 따라 올바른 과거 버전을 찾는지 확인

---

## Phase 7: Lock 매니저

### 학습 목표
- InnoDB의 row-level locking과 deadlock detection을 이해한다
- 2PL (Two-Phase Locking) 프로토콜을 구현한다

### InnoDB 원본 참고
- `storage/innobase/lock/` — 락 시스템
- `lock0lock.cc` — 락 관리 핵심
- InnoDB는 record lock, gap lock, next-key lock을 사용

### 구현 항목

| 항목 | 설명 |
|------|------|
| `LockMode` | SHARED (S), EXCLUSIVE (X) |
| `LockTarget` | 어떤 레코드에 대한 락인지 |
| `LockManager` | 락 획득, 대기, 해제 관리 |
| `WaitForGraph` | deadlock detection용 대기 그래프 |
| `DeadlockDetector` | 사이클 탐지 → 하나를 abort |

### Rust 구현 가이드

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
enum LockMode {
    Shared,     // 읽기 락 — 여러 트랜잭션이 동시에 보유 가능
    Exclusive,  // 쓰기 락 — 하나만 보유 가능
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LockTarget {
    table_id: u32,
    page_id: PageId,
    record_key: Key,
}

struct LockRequest {
    tx_id: TxId,
    target: LockTarget,
    mode: LockMode,
}

struct LockManager {
    /// target → 현재 보유 중인 락 목록
    lock_table: Mutex<HashMap<LockTarget, Vec<GrantedLock>>>,
    /// target → 대기 중인 락 요청 큐
    wait_queue: Mutex<HashMap<LockTarget, VecDeque<LockRequest>>>,
}

impl LockManager {
    /// 락 획득 시도 — 호환되면 즉시 부여, 아니면 대기
    fn acquire(&self, request: LockRequest) -> Result<LockResult> { /* ... */ }

    /// 트랜잭션 종료 시 모든 락 해제 + 대기자 깨우기
    fn release_all(&self, tx_id: TxId) -> Result<()> { /* ... */ }

    /// deadlock 탐지 — wait-for graph에서 사이클 찾기
    fn detect_deadlock(&self) -> Option<TxId> {
        // DFS로 사이클 탐지
        // 사이클 발견 시 비용이 가장 적은 트랜잭션을 victim으로 선택
        todo!()
    }
}
```

### Rust 팁
- `Condvar`를 사용하여 락 대기/깨우기 구현
- Deadlock detection은 별도 스레드에서 주기적으로 실행하거나, 락 대기 시마다 즉시 실행
- `parking_lot` 크레이트의 `Mutex`/`Condvar`가 `std` 버전보다 성능이 좋음

### 검증
- [ ] S-S 호환 (동시 읽기 가능), S-X / X-X 비호환 확인
- [ ] 락 대기 후 해제 시 다음 대기자가 락 획득
- [ ] 의도적 deadlock 생성 → 탐지 후 하나의 트랜잭션 abort
- [ ] 트랜잭션 종료 시 모든 락 해제 확인

---

## Phase 8: 간단한 SQL 인터페이스 (Optional)

### 학습 목표
- 지금까지 구현한 모듈을 통합하여 간단한 SQL 명령을 실행한다
- 이 단계는 선택 사항이며, Phase 1-7의 통합 테스트 성격

### 구현 항목

| 항목 | 설명 |
|------|------|
| SQL Parser | `CREATE TABLE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE` 파싱 |
| Executor | 파싱된 SQL을 실제 스토리지 엔진 호출로 변환 |
| REPL | 대화형 SQL 실행 환경 |

### Rust 구현 가이드

```rust
/// 지원할 SQL 문
enum Statement {
    CreateTable { name: String, columns: Vec<ColumnDef> },
    Insert { table: String, values: Vec<Vec<Value>> },
    Select { table: String, where_clause: Option<Expr> },
    Update { table: String, set: Vec<(String, Value)>, where_clause: Option<Expr> },
    Delete { table: String, where_clause: Option<Expr> },
    Begin,
    Commit,
    Rollback,
}

/// SQL 파서 — sqlparser 크레이트 활용 가능
fn parse(sql: &str) -> Result<Statement> { /* ... */ }

/// 실행 엔진
struct Executor {
    buffer_pool: Arc<BufferPool>,
    log_manager: Arc<LogManager>,
    tx_manager: Arc<TxManager>,
    lock_manager: Arc<LockManager>,
}

impl Executor {
    fn execute(&self, tx: &mut Transaction, stmt: Statement) -> Result<QueryResult> {
        match stmt {
            Statement::Select { .. } => { /* B+Tree 검색 + MVCC 가시성 판단 */ }
            Statement::Insert { .. } => { /* 락 획득 → undo 기록 → redo 기록 → B+Tree 삽입 */ }
            // ...
        }
    }
}
```

### Rust 팁
- SQL 파싱은 `sqlparser-rs` 크레이트 활용 추천 — 직접 구현은 학습 가치는 있지만 시간이 많이 소요
- REPL은 `rustyline` 크레이트로 readline 지원

### 검증
- [ ] `CREATE TABLE → INSERT → SELECT` 기본 흐름 동작
- [ ] `BEGIN → INSERT → ROLLBACK` 시 데이터 미반영 확인
- [ ] 두 세션에서 동시 트랜잭션 실행 시 격리 수준 동작 확인

---

## 권장 크레이트 목록

| 크레이트 | 용도 | Phase |
|----------|------|-------|
| `byteorder` | Big Endian 바이트 직렬화 | 1, 4 |
| `crc32fast` | 페이지 체크섬 | 1 |
| `tempfile` | 테스트용 임시 파일/디렉토리 | 전체 |
| `parking_lot` | 고성능 Mutex, RwLock, Condvar | 2, 7 |
| `crossbeam` | lock-free 자료구조, scoped threads | 2, 6 |
| `sqlparser` | SQL 파싱 (Phase 8에서 선택적) | 8 |
| `rustyline` | REPL 인터페이스 | 8 |
| `tracing` | 구조화된 로깅/디버깅 | 전체 |
| `thiserror` | 에러 타입 정의 | 전체 |

---

## 학습 자료

### InnoDB 내부 구조
- [MySQL Source Code (GitHub)](https://github.com/mysql/mysql-server) — `storage/innobase/`
- Jeremy Cole의 [InnoDB 내부 구조 시리즈](https://blog.jcole.us/innodb/)
- *"MySQL Internals: InnoDB Storage Engine"* (official docs)

### 데이터베이스 이론
- *"Database Internals"* — Alex Petrov (O'Reilly)
- *"Designing Data-Intensive Applications"* — Martin Kleppmann
- CMU 15-445 Database Systems (강의 + 과제 공개)

### Rust 시스템 프로그래밍
- *"Rust for Rustaceans"* — Jon Gjengset
- `unsafe` 코드 없이 최대한 구현하되, 성능이 필요한 부분에서만 선택적 사용

---

## 진행 체크리스트

| Phase | 주제 | 상태 |
|-------|------|------|
| 1 | 디스크 페이지 관리 | ⬜ |
| 2 | 버퍼 풀 | ⬜ |
| 3 | B+Tree 인덱스 | ⬜ |
| 4 | 레코드 포맷 | ⬜ |
| 5 | WAL (Redo Log) | ⬜ |
| 6 | 트랜잭션 & MVCC | ⬜ |
| 7 | Lock 매니저 | ⬜ |
| 8 | SQL 인터페이스 (Optional) | ⬜ |

> 각 Phase는 독립적으로 테스트 가능하도록 설계되었으나,
> Phase 3 이후부터는 이전 Phase의 구현에 의존합니다.
> Phase 1-2를 탄탄하게 구현하는 것이 이후 단계의 기반이 됩니다.
