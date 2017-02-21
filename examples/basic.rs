extern crate uuid;
extern crate symbolserver;

use std::env;
use std::fs;
use std::path::Path;
use std::io::Write;

use uuid::Uuid;

use symbolserver::sdk::Sdk;
use symbolserver::memdb::MemDb;
use symbolserver::Result;

fn do_main() -> Result<()> {
    let args : Vec<_> = env::args().collect();
    if args.len() > 1 {
        let p = Path::new(&args[1]);
        let out = fs::File::create("/tmp/test.memdb")?;
        let sdk = Sdk::new(p)?;
        let mut symout = fs::File::create("/tmp/symbols")?;

        for obj_res in sdk.objects()? {
            let (_, obj) = obj_res?;
            for var in obj.variants() {
                let mut symbols = obj.symbols(var.arch())?;
                for (_, sym) in symbols.iter() {
                    symout.write_all(sym.as_bytes())?;
                    symout.write_all(b"\n")?;
                }
            }
        }
        sdk.dump_memdb(out)?;
    }

    let mdb = MemDb::from_path("/tmp/test.memdb")?;
    let sym = mdb.lookup_by_uuid(&"63d32ddb-095d-3974-afc9-8a6cf7c8bbd6".parse::<Uuid>().unwrap(), 6815851772);
    println!("{:?}", sym);

    Ok(())
}

fn main() {
    do_main().unwrap();
}
