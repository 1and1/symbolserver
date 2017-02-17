use std::io::Cursor;
use std::path::Path;
use std::borrow::Cow;

use memmap;
use uuid::Uuid;
use mach_object::{OFile, Symbol, Section, SymbolIter, SymbolReader, DyLib,
    LoadCommand, MachCommand, get_arch_name_from_types, get_arch_from_flag,
    SEG_TEXT, SECT_TEXT, cpu_type_t, cpu_subtype_t};

use super::{Result, Error, ErrorKind};


enum Backing<'a> {
    Buf(Cow<'a, [u8]>),
    Mmap(memmap::Mmap),
}

pub struct Object<'a> {
    backing: Backing<'a>,
    ofile: OFile,
    variants: Vec<Variant>,
}

pub struct SymbolIterator<'a> {
    iter: Option<SymbolIter<'a>>,
}

pub struct Symbols<'a> {
    ofile: &'a OFile,
    cursor: Cursor<&'a [u8]>,
}

pub struct Variant {
    cputype: cpu_type_t,
    cpusubtype: cpu_subtype_t,
    uuid: Option<Uuid>,
    name: Option<String>,
    vmaddr: u64,
    vmsize: u64,
}

impl<'a> Symbols<'a> {
    pub fn iter(&'a mut self) -> SymbolIterator<'a> {
        SymbolIterator {
            iter: self.ofile.symbols(&mut self.cursor),
        }
    }
}

impl<'a> Iterator for SymbolIterator<'a> {
    type Item = (u64, &'a str);

    fn next(&mut self) -> Option<(u64, &'a str)> {
        let iter = try_opt!(self.iter.as_mut());
        while let Some(sym) = iter.next() {
            if let Symbol::Defined { ref name, external, ref section, entry, .. } = sym {
                if !external && name.is_some() {
                    if let &Some(ref sect) = section {
                        let Section { ref sectname, ref segname, .. } = **sect;
                        if segname == SEG_TEXT && sectname == SECT_TEXT {
                            return Some((entry as u64, name.unwrap()));
                        }
                    }
                }
            }
        }
        None
    }
}

impl<'a> Backing<'a> {

    #[inline(always)]
    pub fn buffer(&self) -> &[u8] {
        match *self {
            Backing::Buf(ref buf) => buf,
            Backing::Mmap(ref mmap) => unsafe { mmap.as_slice() }
        }
    }

    #[inline(always)]
    pub fn cursor(&self, offset: usize) -> Cursor<&[u8]> {
        Cursor::new(&self.buffer()[offset..])
    }
}

fn extract_variant<'a>(variants: &'a mut Vec<Variant>, file: &'a OFile) {
    if let &OFile::MachFile { ref header, ref commands, .. } = file {
        let mut variant_uuid = None;
        let mut variant_name = None;
        let mut variant_vmaddr = 0;
        let mut variant_vmsize = 0;
        for &MachCommand(ref load_cmd, _) in commands {
            match load_cmd {
                &LoadCommand::Uuid(uuid) => {
                    variant_uuid = Some(uuid);
                },
                &LoadCommand::IdDyLib(DyLib { ref name, .. }) => {
                    variant_name = Some(name.1.clone());
                }
                &LoadCommand::Segment { ref segname, vmaddr, vmsize, .. } => {
                    if segname == "__TEXT" {
                        variant_vmaddr = vmaddr as u64;
                        variant_vmsize = vmsize as u64;
                    }
                }
                &LoadCommand::Segment64 { ref segname, vmaddr, vmsize, .. } => {
                    if segname == "__TEXT" {
                        variant_vmaddr = vmaddr as u64;
                        variant_vmsize = vmsize as u64;
                    }
                }
                _ => {}
            }
        }
        variants.push(Variant {
            cputype: header.cputype,
            cpusubtype: header.cpusubtype,
            uuid: variant_uuid,
            name: variant_name,
            vmaddr: variant_vmaddr,
            vmsize: variant_vmsize,
        })
    }
}

impl<'a> Object<'a> {

    fn from_backing(backing: Backing<'a>) -> Result<Object<'a>> {
        let ofile = OFile::parse(&mut backing.cursor(0))?;
        let mut variants = vec![];

        match ofile {
            OFile::FatFile { ref files, .. } => {
                for &(_, ref file) in files {
                    extract_variant(&mut variants, file);
                }
            }
            OFile::MachFile { .. } => {
                extract_variant(&mut variants, &ofile);
            }
            _ => {}
        }

        Ok(Object {
            backing: backing,
            ofile: ofile,
            variants: variants,
        })
    }

    fn from_cow(cow: Cow<'a, [u8]>) -> Result<Object<'a>> {
        Object::from_backing(Backing::Buf(cow))
    }

    /// Parses a macho object from a given slice.
    pub fn from_slice(buf: &'a [u8]) -> Result<Object<'a>> {
        Object::from_cow(Cow::Borrowed(buf))
    }

    /// Parses a macho object from a vector.
    pub fn from_vec(buf: Vec<u8>) -> Result<Object<'a>> {
        Object::from_cow(Cow::Owned(buf))
    }

    /// Parses a macho object from a memory mapped file.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Object<'a>> {
        let mmap = memmap::Mmap::open_path(path, memmap::Protection::Read)?;
        Object::from_backing(Backing::Mmap(mmap))
    }

    /// Return a slice of the variants
    pub fn variants(&'a self) -> &'a [Variant] {
        &self.variants[..]
    }

    /// Returns an iterator over the symbols of an architecture.
    pub fn symbols<'b>(&'a self, arch: &'b str) -> Result<Symbols<'a>> {
        let &(cputype, cpusubtype) = get_arch_from_flag(arch).ok_or_else(|| {
            Error::from(ErrorKind::UnknownArchitecture(arch.to_string()))
        })?;

        match self.ofile {
            OFile::FatFile { ref files, .. } => {
                for &(ref arch, ref file) in files {
                    if arch.cputype == cputype && arch.cpusubtype == cpusubtype {
                        return Ok(Symbols {
                            ofile: file,
                            cursor: self.backing.cursor(arch.offset as usize),
                        });
                    }
                }
            }
            OFile::MachFile { ref header, .. } => {
                if header.cputype == cputype && header.cpusubtype == cpusubtype {
                    return Ok(Symbols {
                        ofile: &self.ofile,
                        cursor: self.backing.cursor(0),
                    });
                }
            }
            _ => {}
        }

        return Err(ErrorKind::MissingArchitecture(arch.to_string()).into());
    }
}

impl Variant {
    pub fn arch(&self) -> &str {
        get_arch_name_from_types(self.cputype, self.cpusubtype).unwrap_or("unknown")
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(|x| x.as_str())
    }

    pub fn uuid(&self) -> Option<Uuid> {
        self.uuid
    }

    pub fn vmaddr(&self) -> u64 {
        self.vmaddr
    }

    pub fn vmsize(&self) -> u64 {
        self.vmsize
    }
}

pub fn test() {
    let obj = Object::from_path("/Users/mitsuhiko/Library/Developer/Xcode/iOS DeviceSupport/10.2 (14C92)/Symbols/System/Library/CoreServices/Encodings/libKoreanConverter.dylib").unwrap();

    for variant in obj.variants() {
        let mut syms = obj.symbols(variant.arch()).unwrap();
        for (addr, sym) in syms.iter() {
            println!("{} | {} | {} | {}", variant.name().unwrap_or("?"), variant.arch(), addr, sym);
        }
    }
}
