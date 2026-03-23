use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use std::io::Cursor;

/// B+Tree 검색 키
///
/// InnoDB에서 Clustered Index의 키는 Primary Key이다.
/// 단순화를 위해 가변 길이 바이트 배열로 표현한다.
/// Big Endian으로 저장하므로 바이트 단위 비교가 곧 논리적 비교와 같다.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key(Vec<u8>);

impl Key {
    pub fn new(data: Vec<u8>) -> Self {
        Self(data)
    }

    /// u64 값으로부터 키 생성 (Big Endian 8바이트)
    pub fn from_u64(value: u64) -> Self {
        let mut buf = Vec::with_capacity(8);
        buf.write_u64::<BigEndian>(value).unwrap();
        Self(buf)
    }

    /// u64 값으로 변환
    pub fn as_u64(&self) -> Option<u64> {
        if self.0.len() != 8 {
            return None;
        }
        let mut cursor = Cursor::new(&self.0);
        Some(cursor.read_u64::<BigEndian>().unwrap())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 바이트 배열로부터 키 생성
    pub fn from_bytes(data: &[u8]) -> Self {
        Self(data.to_vec())
    }

    /// 직렬화된 크기 (2바이트 길이 접두사 + 데이터)
    pub fn serialized_size(&self) -> usize {
        2 + self.0.len()
    }

    /// 바이트 버퍼에 직렬화 (길이 접두사 + 데이터)
    pub fn serialize_into(&self, buf: &mut Vec<u8>) {
        buf.write_u16::<BigEndian>(self.0.len() as u16).unwrap();
        buf.extend_from_slice(&self.0);
    }

    /// 바이트 슬라이스에서 역직렬화, 소비한 바이트 수 반환
    pub fn deserialize_from(data: &[u8]) -> (Self, usize) {
        let mut cursor = Cursor::new(data);
        let len = cursor.read_u16::<BigEndian>().unwrap() as usize;
        let key = Self(data[2..2 + len].to_vec());
        (key, 2 + len)
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(v) = self.as_u64() {
            write!(f, "Key({})", v)
        } else {
            write!(f, "Key({:?})", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_u64_key_ordering() {
        let k1 = Key::from_u64(1);
        let k2 = Key::from_u64(2);
        let k100 = Key::from_u64(100);

        assert!(k1 < k2);
        assert!(k2 < k100);
        assert_eq!(k1, Key::from_u64(1));
    }

    #[test]
    fn test_u64_roundtrip() {
        let key = Key::from_u64(42);
        assert_eq!(key.as_u64(), Some(42));
    }

    #[test]
    fn test_serialize_deserialize() {
        let key = Key::from_u64(12345);
        let mut buf = Vec::new();
        key.serialize_into(&mut buf);

        let (decoded, consumed) = Key::deserialize_from(&buf);
        assert_eq!(decoded, key);
        assert_eq!(consumed, key.serialized_size());
    }
}
