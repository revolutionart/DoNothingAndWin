use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::{collections::HashMap, io::Read};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Chunk {
    pub guid: String,
    pub rolling: u64,
    #[serde(default)]
    pub rolling_only: bool,
    pub sha: [u8; 20],
    pub group: u8,
    pub window: u32,
    pub size: u64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Part {
    pub guid: String,
    pub offset: u32,
    pub size: u32,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    pub sha: [u8; 20],
    pub tags: Vec<String>,
    pub parts: Vec<Part>,
}
impl File {
    pub fn size(&self) -> u64 {
        self.parts.iter().map(|p| p.size as u64).sum()
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub app: String,
    pub build: String,
    pub feature: u32,
    pub files: Vec<File>,
    pub chunks: HashMap<String, Chunk>,
    pub prereq: String,
}
impl Chunk {
    pub fn path(&self, feature: u32) -> String {
        format!(
            "{}/{:02}/{:016X}_{}.chunk",
            if feature >= 22 {
                "ChunksV5"
            } else if feature >= 15 {
                "ChunksV4"
            } else if feature >= 6 {
                "ChunksV3"
            } else {
                "ChunksV2"
            },
            self.group,
            self.rolling,
            self.guid
        )
    }
}
pub fn digest(b: &[u8]) -> [u8; 20] {
    Sha1::digest(b).into()
}
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn blob<const N: usize>(v: &serde_json::Value) -> Result<[u8; N], String> {
    let s = v.as_str().ok_or("Missing JSON manifest blob")?;
    if s.len() != N * 3 || !s.is_ascii() {
        return Err("Invalid JSON manifest blob length".into());
    }
    let mut out = [0; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = s[i * 3..i * 3 + 3]
            .parse()
            .map_err(|_| "Invalid JSON manifest byte")?;
    }
    Ok(out)
}
fn parse_json(bytes: &[u8]) -> Result<Manifest, String> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "Invalid JSON manifest")?;
    if v["bIsFileData"].as_bool() != Some(false) {
        return Err("File-data manifests are unsupported".into());
    }
    let mut m = Manifest {
        app: v["AppNameString"]
            .as_str()
            .ok_or("Missing app identity")?
            .into(),
        build: v["BuildVersionString"]
            .as_str()
            .ok_or("Missing build identity")?
            .into(),
        feature: u32::from_le_bytes(blob(&v["ManifestFileVersion"])?),
        ..Default::default()
    };
    for (guid, hash) in v["ChunkHashList"]
        .as_object()
        .ok_or("Missing chunk hashes")?
    {
        if guid.len() != 32 || !guid.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid chunk identity".into());
        }
        let rolling_only = v.get("ChunkShaList").is_none();
        let mut digest = [0u8; 20];
        if !rolling_only {
            let sha = v["ChunkShaList"][guid]
                .as_str()
                .ok_or("Missing chunk SHA1")?;
            if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("Invalid chunk SHA1".into());
            }
            for (i, b) in digest.iter_mut().enumerate() {
                *b = u8::from_str_radix(&sha[i * 2..i * 2 + 2], 16).map_err(|_| "Invalid SHA1")?;
            }
        }
        let chunk = Chunk {
            guid: guid.to_uppercase(),
            rolling: u64::from_le_bytes(blob(hash)?),
            sha: digest,
            rolling_only,
            group: blob::<1>(&v["DataGroupList"][guid])?[0],
            window: 1024 * 1024,
            size: if v.get("ChunkFilesizeList").is_none() {
                1024 * 1024
            } else {
                u64::from_le_bytes(blob(&v["ChunkFilesizeList"][guid])?)
            },
        };
        m.chunks.insert(chunk.guid.clone(), chunk);
    }
    let mut names = std::collections::HashSet::new();
    for f in v["FileManifestList"]
        .as_array()
        .ok_or("Missing file list")?
    {
        let name = f["Filename"].as_str().ok_or("Missing filename")?.to_owned();
        crate::installer::relative(&name)?;
        if !names.insert(name.to_lowercase()) {
            return Err("Duplicate manifest filename".into());
        }
        let mut file = File {
            name,
            sha: blob(&f["FileHash"])?,
            ..Default::default()
        };
        for p in f["FileChunkParts"]
            .as_array()
            .ok_or("Missing file chunks")?
        {
            let part = Part {
                guid: p["Guid"]
                    .as_str()
                    .ok_or("Missing chunk identity")?
                    .to_uppercase(),
                offset: u32::from_le_bytes(blob(&p["Offset"])?),
                size: u32::from_le_bytes(blob(&p["Size"])?),
            };
            let chunk = m.chunks.get(&part.guid).ok_or("Unknown chunk")?;
            if part.offset as u64 + part.size as u64 > chunk.window as u64 {
                return Err("Chunk part exceeds window".into());
            }
            file.parts.push(part);
        }
        m.files.push(file);
    }
    Ok(m)
}
#[cfg(test)]
mod json_tests {
    use super::*;
    #[test]
    fn legacy_blobs_are_little_endian_and_bounded() {
        assert_eq!(
            u32::from_le_bytes(blob(&serde_json::json!("012000000000")).unwrap()),
            12
        );
        assert!(blob::<1>(&serde_json::json!("256")).is_err());
        assert!(blob::<4>(&serde_json::json!("012")).is_err());
    }
    #[test]
    fn legacy_manifest_rejects_escaping_files_and_file_data() {
        let mut v = serde_json::json!({"ManifestFileVersion":"012000000000","bIsFileData":false,"AppNameString":"Fixture","BuildVersionString":"1","ChunkHashList":{},"FileManifestList":[]});
        assert_eq!(parse(&serde_json::to_vec(&v).unwrap()).unwrap().feature, 12);
        v["FileManifestList"] = serde_json::json!([{"Filename":"../escape","FileHash":"000000000000000000000000000000000000000000000000000000000000","FileChunkParts":[]}]);
        assert!(parse(&serde_json::to_vec(&v).unwrap()).is_err());
        v["bIsFileData"] = serde_json::json!(true);
        assert!(parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}
impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let e = self.p.checked_add(n).ok_or("Manifest length overflow")?;
        let v = self.b.get(self.p..e).ok_or("Truncated manifest")?;
        self.p = e;
        Ok(v)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn sha(&mut self) -> Result<[u8; 20], String> {
        Ok(self.take(20)?.try_into().unwrap())
    }
    fn count(&mut self) -> Result<usize, String> {
        let n = self.u32()? as usize;
        if n > 1_000_000 || n > self.b.len() - self.p {
            return Err("Invalid manifest count".into());
        }
        Ok(n)
    }
    fn guid(&mut self) -> Result<String, String> {
        Ok(format!(
            "{:08X}{:08X}{:08X}{:08X}",
            self.u32()?,
            self.u32()?,
            self.u32()?,
            self.u32()?
        ))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u32()? as i32;
        if n == 0 {
            return Ok(String::new());
        }
        if n.unsigned_abs() > 1_048_576 {
            return Err("Manifest string too long".into());
        }
        if n < 0 {
            let b = self.take(n.unsigned_abs() as usize * 2)?;
            if b[b.len() - 2..] != [0, 0] {
                return Err("Invalid string terminator".into());
            }
            String::from_utf16(
                &b[..b.len() - 2]
                    .chunks_exact(2)
                    .map(|p| u16::from_le_bytes([p[0], p[1]]))
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| "Invalid UTF16".into())
        } else {
            let b = self.take(n as usize)?;
            if b[b.len() - 1] != 0 {
                return Err("Invalid string terminator".into());
            }
            String::from_utf8(b[..b.len() - 1].to_vec()).map_err(|_| "Invalid UTF8".into())
        }
    }
    fn section(&mut self) -> Result<Reader<'a>, String> {
        let len = self.u32()? as usize;
        if len < 5 {
            return Err("Invalid manifest section".into());
        }
        Ok(Reader::new(self.take(len - 4)?))
    }
}
fn decompress(bytes: &[u8], size: usize) -> Result<Vec<u8>, String> {
    if size > 512 * 1024 * 1024 {
        return Err("Manifest or chunk exceeds size limit".into());
    }
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(bytes)
        .take(size as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| "Invalid compressed data")?;
    if out.len() != size {
        return Err("Decompressed size mismatch".into());
    }
    Ok(out)
}
pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
    if bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{') {
        return parse_json(bytes);
    }
    let mut h = Reader::new(bytes);
    if h.u32()? != 0x44bec00c {
        return Err("Unsupported non-binary manifest".into());
    }
    let header = h.u32()? as usize;
    let raw = h.u32()? as usize;
    let packed = h.u32()? as usize;
    let sha = h.sha()?;
    let flags = h.u8()?;
    let version = h.u32()?;
    if !(14..=21).contains(&version) || flags > 1 {
        return Err(format!(
            "Unsupported manifest format {version} / flags {flags}; installation stopped."
        ));
    }
    if header < h.p || bytes.len() != header + packed {
        return Err("Manifest header size mismatch".into());
    }
    let payload = bytes.get(header..).ok_or("Invalid manifest header")?;
    let data = if flags == 1 {
        decompress(payload, raw)?
    } else {
        payload.to_vec()
    };
    if data.len() != raw || digest(&data) != sha {
        return Err("Manifest integrity check failed".into());
    }
    let mut r = Reader::new(&data);
    let mut m = r.section()?;
    let metadata_version = m.u8()?;
    if metadata_version > 2 {
        return Err("Unsupported manifest metadata version".into());
    }
    let feature = m.u32()?;
    if m.u8()? != 0 {
        return Err("File-data manifests are not supported".into());
    }
    let _app_id = m.u32()?;
    let app = m.string()?;
    let build = m.string()?;
    let _exe = m.string()?;
    let _args = m.string()?;
    for _ in 0..m.count()? {
        m.string()?;
    }
    m.string()?;
    let prereq = m.string()?;
    m.string()?;
    let mut c = r.section()?;
    if c.u8()? > 0 {
        return Err("Unsupported chunk list version".into());
    }
    let n = c.count()?;
    let mut chunks = vec![Chunk::default(); n];
    for x in &mut chunks {
        x.guid = c.guid()?;
    }
    for x in &mut chunks {
        x.rolling = c.u64()?;
    }
    for x in &mut chunks {
        x.sha = c.sha()?;
    }
    for x in &mut chunks {
        x.group = c.u8()?;
    }
    for x in &mut chunks {
        x.window = c.u32()?;
        if x.window == 0 || x.window > 64 * 1024 * 1024 {
            return Err("Invalid chunk window".into());
        }
    }
    for x in &mut chunks {
        x.size = c.u64()?;
        if x.size > 64 * 1024 * 1024 {
            return Err("Invalid chunk size".into());
        }
    }
    if c.p != c.b.len() {
        return Err("Chunk list length mismatch".into());
    }
    let mut f = r.section()?;
    let fv = f.u8()?;
    if fv > 2 {
        return Err("Unsupported file list version".into());
    }
    let n = f.count()?;
    let mut files = vec![File::default(); n];
    for x in &mut files {
        x.name = f.string()?;
    }
    for _ in &files {
        if !f.string()?.is_empty() {
            return Err("Symlink manifest entries are unsupported on Windows".into());
        }
    }
    for x in &mut files {
        x.sha = f.sha()?;
    }
    for _ in &files {
        f.u8()?;
    }
    for x in &mut files {
        for _ in 0..f.count()? {
            x.tags.push(f.string()?);
        }
    }
    for x in &mut files {
        for _ in 0..f.count()? {
            let mut p = f.section()?;
            x.parts.push(Part {
                guid: p.guid()?,
                offset: p.u32()?,
                size: p.u32()?,
            });
            if p.p != p.b.len() {
                return Err("Chunk part length mismatch".into());
            }
        }
    }
    if fv >= 1 {
        for _ in &files {
            if f.u32()? != 0 {
                f.take(16)?;
            }
        }
        for _ in &files {
            f.string()?;
        }
    }
    if fv >= 2 {
        for _ in &files {
            f.take(32)?;
        }
    }
    if f.p != f.b.len() {
        return Err("File list length mismatch".into());
    }
    let result = Manifest {
        app,
        build,
        feature,
        files,
        chunks: chunks.into_iter().map(|x| (x.guid.clone(), x)).collect(),
        prereq,
    };
    for file in &result.files {
        for p in &file.parts {
            let ch = result
                .chunks
                .get(&p.guid)
                .ok_or("Unknown chunk reference")?;
            if p.offset as u64 + p.size as u64 > ch.window as u64 {
                return Err("Chunk part exceeds window".into());
            }
        }
    }
    if result.files.is_empty() {
        return Err("Empty engine manifest".into());
    }
    Ok(result)
}
// Legacy JSON manifests omit chunk SHA1; final assembled files still require SHA1.
pub fn valid_chunk_data(data: &[u8], expected: &Chunk) -> bool {
    data.len() == expected.window as usize
        && if expected.rolling_only {
            rolling_hash(data) == expected.rolling
        } else {
            digest(data) == expected.sha
        }
}
fn rolling_hash(data: &[u8]) -> u64 {
    static TABLE: std::sync::OnceLock<[u64; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        std::array::from_fn(|i| {
            let mut value = i as u64;
            for _ in 0..8 {
                value = (value >> 1)
                    ^ if value & 1 != 0 {
                        0xC96C5795D7870F42
                    } else {
                        0
                    };
            }
            value
        })
    });
    data.iter()
        .fold(0u64, |hash, b| hash.rotate_left(1) ^ table[*b as usize])
}
pub fn decode_chunk(bytes: &[u8], expected: &Chunk) -> Result<Vec<u8>, String> {
    let mut r = Reader::new(bytes);
    if r.u32()? != 0xb1fe3aa2 {
        return Err("Invalid chunk magic".into());
    }
    let version = r.u32()?;
    let header = r.u32()? as usize;
    let size = r.u32()? as usize;
    let guid = r.guid()?;
    let rolling = r.u64()?;
    let flags = r.u8()?;
    if !(1..=3).contains(&version) || flags > 1 {
        return Err("Unsupported chunk encoding".into());
    }
    if version >= 2 {
        r.take(21)?;
    }
    if version >= 3 && r.u32()? != expected.window {
        return Err("Chunk window mismatch".into());
    }
    if header < r.p
        || header.checked_add(size) != Some(bytes.len())
        || guid != expected.guid
        || rolling != expected.rolling
    {
        return Err("Chunk identity or size mismatch".into());
    }
    let b = &bytes[header..];
    let data = if flags == 1 {
        decompress(b, expected.window as usize)?
    } else {
        b.to_vec()
    };
    if !valid_chunk_data(&data, expected) {
        return Err("Chunk integrity verification failed".into());
    }
    Ok(data)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_manifest_and_chunk_without_sha_are_verified() {
        fn blob(b: &[u8]) -> String {
            b.iter().map(|b| format!("{b:03}")).collect()
        }
        let data = vec![42u8; 1024 * 1024];
        let guid = "00000001000000020000000300000004";
        let mut v = serde_json::json!({
            "ManifestFileVersion":"007000000000", "bIsFileData":false,
            "AppNameString":"UE_4.0", "BuildVersionString":"4.0.2",
            "ChunkHashList":{guid:blob(&rolling_hash(&data).to_le_bytes())},
            "DataGroupList":{guid:"001"},
            "FileManifestList":[{"Filename":"test.txt","FileHash":blob(&digest(&data)),
                "FileChunkParts":[{"Guid":guid,"Offset":"000000000000","Size":blob(&(data.len() as u32).to_le_bytes())}]}]
        });
        let m = parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        let ch = &m.chunks[guid];
        assert!(ch.rolling_only);
        assert_eq!(ch.size, 1024 * 1024);
        let mut bytes = Vec::new();
        for n in [0xb1fe3aa2u32, 1, 41, data.len() as u32, 1, 2, 3, 4] {
            bytes.extend(n.to_le_bytes());
        }
        bytes.extend(ch.rolling.to_le_bytes());
        bytes.push(0);
        bytes.extend(&data);
        assert_eq!(decode_chunk(&bytes, ch).unwrap(), data);
        *bytes.last_mut().unwrap() ^= 1;
        assert!(decode_chunk(&bytes, ch).is_err());
        v["ChunkShaList"] = serde_json::json!({guid:"invalid"});
        assert!(parse(&serde_json::to_vec(&v).unwrap()).is_err());
        v["ChunkShaList"] = serde_json::json!({guid:hex(&digest(&data))});
        let modern = parse(&serde_json::to_vec(&v).unwrap()).unwrap();
        assert!(!modern.chunks[guid].rolling_only);
        assert!(valid_chunk_data(&data, &modern.chunks[guid]));
        v["ChunkFilesizeList"] = serde_json::json!({guid:"bad"});
        assert!(parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
    #[test]
    fn malformed_inputs_fail_without_panics() {
        for n in 0..128 {
            assert!(parse(&vec![0; n]).is_err());
            assert!(decode_chunk(&vec![0; n], &Chunk::default()).is_err());
        }
    }
    #[test]
    fn chunk_hash_and_identity_enforced() {
        let data = b"test payload";
        let ch = Chunk {
            guid: "00000001000000020000000300000004".into(),
            rolling: 123,
            sha: digest(data),
            window: data.len() as u32,
            ..Default::default()
        };
        let mut b = Vec::new();
        for n in [0xb1fe3aa2u32, 3, 66, data.len() as u32, 1, 2, 3, 4] {
            b.extend(n.to_le_bytes());
        }
        b.extend(123u64.to_le_bytes());
        b.push(0);
        b.extend(ch.sha);
        b.push(3);
        b.extend(ch.window.to_le_bytes());
        b.extend(data);
        assert_eq!(decode_chunk(&b, &ch).unwrap(), data);
        *b.last_mut().unwrap() ^= 1;
        assert!(decode_chunk(&b, &ch).is_err());
    }
}
