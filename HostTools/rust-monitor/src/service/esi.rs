// SPDX-License-Identifier: GPL-3.0-only
//! Rust port of the PDO-category preparation and verified burn workflow in
//! SimpleCAT src/esi_flash.c and src/tc3_scan.c (7bf6be13).
use super::{helper, number, process::Wire, Result};
use crate::backend::Backend;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use xml::{
    attribute::OwnedAttribute,
    reader::{EventReader, XmlEvent},
};

const CATEGORY_START: usize = 0x40;
const TX_PDO: u16 = 50;
const RX_PDO: u16 = 51;

#[derive(Default)]
struct Node {
    name: String,
    attributes: Vec<OwnedAttribute>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |n| n.name == name)
    }

    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|n| n.name == name)
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|a| a.name.local_name == name)
            .map(|a| a.value.as_str())
    }

    fn text_number<T: TryFrom<u64>>(&self, name: &str) -> Result<T> {
        self.child(name)
            .and_then(|n| number(n.text.trim()))
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| format!("Invalid or missing ESI {name}."))
    }
}

fn parse_xml(input: &[u8]) -> Result<Node> {
    let mut stack = vec![Node::default()];
    let mut nodes = 0;
    for event in EventReader::new(input) {
        match event.map_err(|e| format!("Invalid ESI XML: {e}"))? {
            XmlEvent::StartElement {
                name, attributes, ..
            } => {
                nodes += 1;
                if stack.len() >= 64 || nodes >= 100_000 {
                    return Err("ESI XML exceeds parser limits.".into());
                }
                stack.push(Node {
                    name: name.local_name,
                    attributes,
                    ..Node::default()
                });
            }
            XmlEvent::EndElement { .. } => {
                let node = stack.pop().ok_or("Malformed ESI XML.")?;
                stack
                    .last_mut()
                    .ok_or("Malformed ESI XML.")?
                    .children
                    .push(node);
            }
            XmlEvent::Characters(text) | XmlEvent::CData(text) => {
                stack
                    .last_mut()
                    .ok_or("Malformed ESI XML.")?
                    .text
                    .push_str(&text);
            }
            _ => {}
        }
    }
    stack
        .pop()
        .filter(|_| stack.is_empty())
        .ok_or("Incomplete ESI XML.".into())
}

fn dword(words: &[u16], offset: usize) -> u32 {
    u32::from(words[offset]) | (u32::from(words[offset + 1]) << 16)
}

fn capacity(words: &[u16]) -> Result<usize> {
    words
        .get(0x3e)
        .filter(|value| **value < 1024)
        .map(|value| (usize::from(*value) + 1) * 64)
        .filter(|words| *words > CATEGORY_START)
        .ok_or("Unsupported SII capacity.".into())
}

fn template_device<'a>(xml: &'a Node, snapshot: &[u16]) -> Result<&'a Node> {
    let root = xml
        .child("EtherCATInfo")
        .ok_or("Expected EtherCATInfo in ESI.")?;
    let vendor = dword(snapshot, 8);
    let product = dword(snapshot, 10);
    let revision = dword(snapshot, 12);
    let xml_vendor = root
        .child("Vendor")
        .ok_or("Expected ESI Vendor.")?
        .text_number::<u32>("Id")?;
    if vendor == 0 || product == 0 || vendor != xml_vendor {
        return Err("ESI vendor does not match the live SII identity.".into());
    }
    let devices = root
        .child("Descriptions")
        .and_then(|n| n.child("Devices"))
        .ok_or("No ESI devices.")?;
    let mut matching = devices.children("Device").filter(|device| {
        let Some(kind) = device.child("Type") else {
            return false;
        };
        kind.attribute("ProductCode").and_then(number) == Some(u64::from(product))
            && kind
                .attribute("RevisionNo")
                .is_none_or(|r| number(r) == Some(u64::from(revision)))
    });
    match (matching.next(), matching.next()) {
        (Some(device), None) => Ok(device),
        _ => Err(format!(
            "ESI must match exactly one live product/revision {product:08X}:{revision:08X}."
        )),
    }
}

fn type_code(data: &str) -> u8 {
    match data {
        "BOOL" | "BOOLEAN" => 1,
        "SINT" => 2,
        "INT" => 3,
        "DINT" => 4,
        "USINT" | "BYTE" => 5,
        "UINT" | "WORD" => 6,
        "UDINT" | "DWORD" => 7,
        "REAL" => 8,
        "LREAL" => 0x11,
        "LINT" => 0x15,
        "ULINT" | "LWORD" => 0x1b,
        _ => 0,
    }
}

fn pdo_category(device: &Node, direction: &str, sync_managers: &[(u8, u16)]) -> Result<Vec<u16>> {
    let mut data = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let output = direction == "RxPdo";
    let mut widths = vec![0usize; sync_managers.len()];
    for pdo in device.children(direction) {
        let index = pdo.text_number::<u16>("Index")?;
        let valid_range = if output {
            0x1600..=0x17ff
        } else {
            0x1a00..=0x1bff
        };
        if !valid_range.contains(&index) || !seen.insert(index) {
            return Err(format!("Invalid or duplicate {direction} 0x{index:04x}."));
        }
        let manager = match pdo.attribute("Sm") {
            Some(sm) => {
                let sm = number(sm)
                    .and_then(|v| u8::try_from(v).ok())
                    .ok_or("Invalid ESI Sm.")?;
                if sync_managers.get(usize::from(sm)).map(|manager| manager.0)
                    != Some(if output { 3 } else { 4 })
                {
                    return Err(format!(
                        "{direction} 0x{index:04x} does not match live SII SM {sm}."
                    ));
                }
                sm
            }
            None => 0xff,
        };
        let count =
            u8::try_from(pdo.children("Entry").count()).map_err(|_| "Too many PDO entries.")?;
        if count == 0 {
            return Err("Empty ESI PDO.".into());
        }
        let flags = if manager == 0xff { 0 } else { 2 };
        data.extend(index.to_le_bytes());
        data.extend([count, manager, 0, 0, flags, 0]);
        for entry in pdo.children("Entry") {
            let index = entry.text_number::<u16>("Index")?;
            let sub = if index == 0 {
                0
            } else {
                entry.text_number::<u8>("SubIndex")?
            };
            let bits = entry.text_number::<u8>("BitLen")?;
            if bits == 0 {
                return Err("Zero-width PDO entry.".into());
            }
            if manager != 0xff {
                widths[usize::from(manager)] += usize::from(bits);
            }
            let dtype = entry
                .child("DataType")
                .map_or(0, |n| type_code(n.text.trim()));
            data.extend(index.to_le_bytes());
            data.extend([sub, 0, dtype, bits, 0, 0]);
        }
    }
    for (sm, ((kind, declared), bits)) in sync_managers.iter().zip(widths).enumerate() {
        if *kind == (if output { 3 } else { 4 })
            && *declared != 0
            && bits.div_ceil(8) != usize::from(*declared)
        {
            return Err(format!(
                "{direction} size for SM {sm} disagrees with live SII ({declared} bytes)."
            ));
        }
    }
    Ok(data
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect())
}

fn categories(words: &[u16]) -> Result<Vec<(u16, &[u16])>> {
    let mut pos = CATEGORY_START;
    let mut result = Vec::new();
    loop {
        let kind = *words.get(pos).ok_or("SII categories lack a terminator.")?;
        if kind == 0xffff {
            return Ok(result);
        }
        let length = usize::from(*words.get(pos + 1).ok_or("Truncated SII category header.")?);
        let end = pos + 2 + length;
        result.push((
            kind,
            words
                .get(pos + 2..end)
                .ok_or("Truncated SII category data.")?,
        ));
        pos = end;
    }
}

fn build(xml: &Node, original: &[u16]) -> Result<Vec<u16>> {
    if original.len() <= CATEGORY_START || original.len() > capacity(original)? {
        return Err("Invalid size for the original SII image.".into());
    }
    let device = template_device(xml, original)?;
    let original_categories = categories(original)?;
    let sm = original_categories
        .iter()
        .filter(|(kind, _)| *kind == 41)
        .map(|(_, data)| *data)
        .collect::<Vec<_>>();
    if sm.len() != 1 || sm[0].len() % 4 != 0 {
        return Err("SII must contain one well-formed SyncManager category.".into());
    }
    let sync_managers: Vec<_> = sm[0]
        .chunks_exact(4)
        .map(|entry| ((entry[3] >> 8) as u8, entry[1]))
        .collect();
    let tx = pdo_category(device, "TxPdo", &sync_managers)?;
    let rx = pdo_category(device, "RxPdo", &sync_managers)?;
    if tx.is_empty() && rx.is_empty() {
        return Err("ESI device contains no TxPdo or RxPdo definitions.".into());
    }
    let mut result = original[..CATEGORY_START].to_vec();
    let mut used = [false; 2];
    for (kind, data) in original_categories {
        let replacement = match kind {
            TX_PDO => {
                if std::mem::replace(&mut used[0], true) {
                    continue;
                }
                &tx[..]
            }
            RX_PDO => {
                if std::mem::replace(&mut used[1], true) {
                    continue;
                }
                &rx[..]
            }
            _ => data,
        };
        append_category(&mut result, kind, replacement)?;
    }
    for (exists, kind, data) in [(used[0], TX_PDO, tx), (used[1], RX_PDO, rx)] {
        if !exists && !data.is_empty() {
            append_category(&mut result, kind, &data)?;
        }
    }
    result.push(0xffff);
    if result.len() > capacity(original)? {
        return Err("ESI categories exceed the target SII capacity.".into());
    }
    Ok(result)
}

fn append_category(image: &mut Vec<u16>, kind: u16, words: &[u16]) -> Result<()> {
    image.push(kind);
    image.push(u16::try_from(words.len()).map_err(|_| "SII category too large.")?);
    image.extend_from_slice(words);
    Ok(())
}

fn save_backup(path: &str, words: &[u16]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create unique SII backup {path}: {e}"))?;
    file.write_all(&bytes(words))
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Cannot persist SII backup: {e}"))
}

fn bytes(words: &[u16]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

fn words(data: &[u8]) -> Result<Vec<u16>> {
    if data.len() % 2 != 0 || data.len() <= CATEGORY_START * 2 || data.len() > 131_072 {
        return Err("Invalid SII image size.".into());
    }
    Ok(data
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect())
}

fn read_esi(path: &str) -> Result<Node> {
    if fs::metadata(path).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
        return Err("ESI XML exceeds 8 MiB.".into());
    }
    parse_xml(&fs::read(path).map_err(|e| e.to_string())?)
}

pub(super) fn prepare(xml: &str, original: &str, prepared: &str) -> Result<()> {
    let source = fs::read(original).map_err(|e| e.to_string())?;
    let image = build(&read_esi(xml)?, &words(&source)?)?;
    save_backup(prepared, &image)?;
    println!("Prepared {} SII words in {prepared}.", image.len());
    Ok(())
}

struct SoemEeprom {
    wire: Wire,
    capacity: usize,
}

impl SoemEeprom {
    fn open(target: &str, position: u16) -> Result<Self> {
        let mut wire =
            Wire::open(helper(Backend::Soem)?.args([target, "--eeprom", &position.to_string()]))?;
        let mut capacity = None;
        wire.response(|row| {
            if let ["capacity", value] = row {
                capacity = value
                    .parse::<usize>()
                    .ok()
                    .filter(|v| (128..=65536).contains(v));
                Ok(())
            } else {
                Err("Unexpected EEPROM status.".into())
            }
        })?;
        Ok(Self {
            wire,
            capacity: capacity.ok_or("Unsupported EEPROM capacity.")?,
        })
    }

    fn word(&mut self, index: usize, write: Option<(u16, u16)>) -> Result<u16> {
        if index >= self.capacity {
            return Err("SII word is outside the EEPROM.".into());
        }
        self.wire.send(&match write {
            Some((old, new)) => format!("W {index} {old} {new}"),
            None => format!("R {index}"),
        })?;
        let mut value = None;
        self.wire.response(|row| {
            if let ["word", address, data] = row {
                if address.parse::<usize>().ok() == Some(index) {
                    value = data.parse::<u16>().ok();
                    return Ok(());
                }
            }
            Err("Unexpected EEPROM word response.".into())
        })?;
        value.ok_or("EEPROM word not returned.".into())
    }

    fn used_image(&mut self) -> Result<Vec<u16>> {
        let mut image = Vec::new();
        for i in 0..CATEGORY_START {
            image.push(self.word(i, None)?);
        }
        loop {
            let kind = self.word(image.len(), None)?;
            image.push(kind);
            if kind == 0xffff {
                return Ok(image);
            }
            let length = self.word(image.len(), None)?;
            image.push(length);
            for _ in 0..length {
                image.push(self.word(image.len(), None)?);
            }
        }
    }
}

fn ethercat(target: &str) -> Command {
    let mut cmd =
        Command::new(std::env::var_os("ETHERCAT_TOOL").unwrap_or_else(|| "ethercat".into()));
    cmd.args(["--master", target]);
    cmd
}

fn result(cmd: &mut Command) -> Result<Vec<u8>> {
    let output = cmd
        .output()
        .map_err(|e| format!("Cannot start IgH ethercat tool: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "IgH tool failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn igh_dump(target: &str, position: &str) -> Result<Vec<u16>> {
    words(&result(ethercat(target).args([
        "--position",
        position,
        "sii_read",
    ]))?)
}

fn igh_idle(target: &str) -> Result<()> {
    let status = result(ethercat(target).arg("master"))?;
    let status = String::from_utf8_lossy(&status);
    if status.lines().any(|line| line.trim() == "Phase: Idle")
        && status.lines().any(|line| line.trim() == "Active: no")
    {
        Ok(())
    } else {
        Err("IgH master must be idle for EEPROM service.".into())
    }
}

fn igh_burn(target: &str, position: &str, before: &[u16], image: &[u16]) -> Result<()> {
    igh_idle(target)?;
    if igh_dump(target, position)? != before {
        return Err("SII changed after planning; aborting write.".into());
    }
    result(ethercat(target).args(["--position", position, "states", "INIT"]))?;
    let mut ready = false;
    for _ in 0..30 {
        let state = result(ethercat(target).args(["--position", position, "slaves"]))?;
        if String::from_utf8_lossy(&state).lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.get(2..4) == Some(&["INIT", "+"][..])
        }) {
            ready = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !ready {
        return Err("IgH target did not reach INIT.".into());
    }
    let mut child = ethercat(target)
        .args(["--position", position, "sii_write", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let write = child
        .stdin
        .take()
        .ok_or("IgH SII input missing.")?
        .write_all(&bytes(image));
    let status = child.wait().map_err(|e| e.to_string())?;
    write.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("IgH SII write failed. Preserve the backup for recovery.".into());
    }
    result(ethercat(target).arg("rescan"))?;
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        if igh_dump(target, position).is_ok_and(|read| read.starts_with(image)) {
            return Ok(());
        }
    }
    Err("IgH SII verification after rescan failed. Preserve the backup for recovery.".into())
}

pub(super) fn run(
    backend: Backend,
    target: &str,
    position: u16,
    xml: &str,
    backup: &str,
    write: bool,
) -> Result<()> {
    run_with_progress(backend, target, position, xml, backup, write, |line| {
        println!("{line}")
    })
}

pub fn run_with_progress(
    backend: Backend,
    target: &str,
    position: u16,
    xml: &str,
    backup: &str,
    write: bool,
    mut progress: impl FnMut(String),
) -> Result<()> {
    let xml = read_esi(xml)?;
    if backend == Backend::Igh {
        let position = position.to_string();
        igh_idle(target)?;
        let original = igh_dump(target, &position)?;
        let image = build(&xml, &original)?;
        save_backup(backup, &original)?;
        progress(format!("已生成 {} 字 SII，备份 {}", image.len(), backup));
        if write && !original.starts_with(&image) {
            igh_burn(target, &position, &original, &image)?;
            progress("IgH 已重扫并核对 SII 镜像。".into());
        }
    } else if backend == Backend::Soem {
        let mut eeprom = SoemEeprom::open(target, position)?;
        let mut original = eeprom.used_image()?;
        let image = build(&xml, &original)?;
        for index in original.len()..image.len() {
            original.push(eeprom.word(index, None)?);
        }
        save_backup(backup, &original)?;
        let changed: Vec<_> = (CATEGORY_START..image.len())
            .filter(|&i| image[i] != original[i])
            .collect();
        progress(format!("{} 字发生变化，备份 {}", changed.len(), backup));
        if write {
            for (n, &index) in changed.iter().rev().enumerate() {
                let value = eeprom.word(index, Some((original[index], image[index])))?;
                if value != image[index] {
                    return Err(format!("SII verification mismatch at word 0x{index:04x}."));
                }
                if (n + 1) % 16 == 0 || n + 1 == changed.len() {
                    progress(format!("已写入并校验 {}/{} 字", n + 1, changed.len()));
                }
            }
        }
        eeprom.wire.finish()?;
    } else {
        return Err("EEPROM service requires a physical backend.".into());
    }
    Ok(())
}

pub fn demo_update(current: &mut Vec<u16>, write: bool) -> Result<usize> {
    let xml = parse_xml(
        br#"<EtherCATInfo><Vendor><Id>#x24121</Id></Vendor>
      <Descriptions><Devices><Device><Type ProductCode="1" RevisionNo="1"/>
      <TxPdo Sm="3"><Index>#x1a00</Index><Entry><Index>#x6000</Index>
      <SubIndex>0</SubIndex><BitLen>32</BitLen><DataType>UDINT</DataType></Entry></TxPdo>
      </Device></Devices></Descriptions></EtherCATInfo>"#,
    )?;
    if current.is_empty() {
        let mut baseline = vec![0; CATEGORY_START];
        baseline[8] = 0x4121;
        baseline[9] = 2;
        baseline[10] = 1;
        baseline[12] = 1;
        baseline[0x3e] = 1;
        append_category(
            &mut baseline,
            41,
            &[
                0, 0, 0, 0x100, 0, 0, 0, 0x200, 0, 0, 0, 0x300, 0, 4, 0, 0x400,
            ],
        )?;
        baseline.push(0xffff);
        *current = baseline;
    }
    let planned = build(&xml, current)?;
    let changed = planned
        .iter()
        .enumerate()
        .filter(|(index, value)| current.get(*index) != Some(value))
        .count();
    if write {
        *current = planned;
        categories(current)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_esi_preview_updates_only_after_mock_write() {
        let mut sii = Vec::new();
        let changed = demo_update(&mut sii, false).unwrap();
        assert!(changed > 0);
        let before = sii.clone();
        assert_eq!(demo_update(&mut sii, false).unwrap(), changed);
        assert_eq!(sii, before);
        assert_eq!(demo_update(&mut sii, true).unwrap(), changed);
        assert_eq!(sii[..CATEGORY_START], before[..CATEGORY_START]);
        assert_eq!(demo_update(&mut sii, false).unwrap(), 0);
    }

    #[test]
    fn esi_build_replaces_pdos_and_preserves_identity_and_other_categories() {
        let xml = parse_xml(
            br#"<EtherCATInfo><Vendor><Id>#x24121</Id></Vendor>
          <Descriptions><Devices><Device><Type ProductCode="1" RevisionNo="2"/>
          <TxPdo Sm="3"><Index>#x1a00</Index><Entry><Index>#x6000</Index>
          <SubIndex>0</SubIndex><BitLen>32</BitLen><DataType>UDINT</DataType></Entry></TxPdo>
          </Device></Devices></Descriptions></EtherCATInfo>"#,
        )
        .unwrap();
        let mut before = vec![0; 64];
        before[8] = 0x4121;
        before[9] = 2;
        before[10] = 1;
        before[12] = 2;
        before[14] = 77;
        before[0x3e] = 1;
        append_category(
            &mut before,
            41,
            &[
                0, 0, 0, 0x100, 0, 0, 0, 0x200, 0, 0, 0, 0x300, 0, 0, 0, 0x400,
            ],
        )
        .unwrap();
        append_category(&mut before, TX_PDO, &[1, 2]).unwrap();
        append_category(&mut before, 63, &[0x1234]).unwrap();
        before.push(0xffff);
        let image = build(&xml, &before).unwrap();
        assert_eq!(image[..64], before[..64]);
        let cats = categories(&image).unwrap();
        assert_eq!(
            cats[1],
            (TX_PDO, &[0x1a00, 0x0301, 0, 2, 0x6000, 0, 0x2007, 0][..])
        );
        assert_eq!(cats[2], (63, &[0x1234][..]));
        before[12] = 3;
        assert!(build(&xml, &before).is_err());
    }
}
