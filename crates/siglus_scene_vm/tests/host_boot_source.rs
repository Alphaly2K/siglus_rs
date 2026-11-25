#![cfg(feature = "art3m1s-host-audio")]

use std::sync::Arc;

use anyhow::{Result, bail};
use siglus_scene_vm::host::{
    SiglusBootSource, SiglusGameexe, SiglusHost, SiglusHostConfig, SiglusSaveStore,
};

#[test]
fn boots_reloads_append_and_reopens_saves_from_host_bytes() {
    use siglus_scene_vm::formats::scene_pck::ScenePckDecodeOptions;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct Source {
        config_reads: AtomicUsize,
        pack_reads: Mutex<Vec<String>>,
    }

    impl SiglusBootSource for Source {
        fn read_gameexe(&self) -> Result<Option<SiglusGameexe>> {
            self.config_reads.fetch_add(1, Ordering::Relaxed);
            Ok(Some(SiglusGameexe::Ini(
                b"#SCREEN_SIZE = 640, 360\n".to_vec(),
            )))
        }

        fn initial_append(&self) -> Result<(String, String)> {
            Ok(("base".into(), "Base".into()))
        }

        fn load_scene_pack(&self, append: &str) -> Result<(Vec<u8>, ScenePckDecodeOptions)> {
            self.pack_reads.lock().unwrap().push(append.to_owned());
            let scene_name = match append {
                "base" => "opening",
                "extra" => "bonus",
                _ => bail!("unknown append {append}"),
            };
            Ok((one_scene_pack(scene_name), ScenePckDecodeOptions::default()))
        }
    }

    #[derive(Default)]
    struct MemorySaves(Mutex<HashMap<String, Vec<u8>>>);

    impl SiglusSaveStore for MemorySaves {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn write(&self, key: &str, bytes: &[u8]) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), bytes.to_vec());
            Ok(())
        }
        fn remove(&self, key: &str) -> Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn one_scene_pack(name: &str) -> Vec<u8> {
        const PACK_HEADER: i32 = 23 * 4;
        const SCENE_HEADER: i32 = 33 * 4;
        let name: Vec<u16> = name.encode_utf16().collect();
        let name_list = PACK_HEADER + 8;
        let data_index = name_list + name.len() as i32 * 2;
        let data_list = data_index + 8;
        let mut header = [0i32; 23];
        header[0] = PACK_HEADER;
        header[13..21].copy_from_slice(&[
            PACK_HEADER,
            1,
            name_list,
            1,
            data_index,
            1,
            data_list,
            1,
        ]);
        use siglus_scene_vm::runtime::{constants::cd, forms::codes::FM_STR};
        let mut script = vec![cd::PUSH];
        script.extend(FM_STR.to_le_bytes());
        script.extend(0i32.to_le_bytes());
        script.extend([cd::NAME, cd::EOF]);
        let script_offset = SCENE_HEADER + 4 + 8 + 2;
        let mut scene = [0i32; 33];
        scene[..7].copy_from_slice(&[
            SCENE_HEADER,
            script_offset,
            script.len() as i32,
            SCENE_HEADER + 4,
            1,
            SCENE_HEADER + 12,
            1,
        ]);
        scene[9..11].copy_from_slice(&[SCENE_HEADER, 1]);
        let mut bytes: Vec<u8> = header.into_iter().flat_map(i32::to_le_bytes).collect();
        bytes.extend(
            [0, name.len() as i32]
                .into_iter()
                .flat_map(i32::to_le_bytes),
        );
        bytes.extend(name.into_iter().flat_map(u16::to_le_bytes));
        bytes.extend(
            [0, script_offset + script.len() as i32]
                .into_iter()
                .flat_map(i32::to_le_bytes),
        );
        bytes.extend(scene.into_iter().flat_map(i32::to_le_bytes));
        bytes.extend([0, 0, 1].into_iter().flat_map(i32::to_le_bytes));
        bytes.extend(u16::from(b'A').to_le_bytes());
        bytes.extend(script);
        bytes
    }

    let source = Arc::new(Source::default());
    let project = std::env::temp_dir().join(format!("art3m1s-siglus-boot-{}", std::process::id()));
    let mut config = SiglusHostConfig::new(project);
    config.scene_id = Some(0);
    let saves = Arc::new(MemorySaves::default());
    config.save_store = Some(saves.clone());
    let mut host =
        SiglusHost::new_external_with_boot_source(config.clone(), source.clone()).unwrap();
    assert_eq!(host.logical_size(), (640, 360));
    let vm = host.vm_mut();
    assert_eq!(vm.ctx.lookup_scene_no("opening").unwrap(), 0);
    vm.ctx.set_active_append("extra".into(), "Extra".into());
    vm.restart_scene_name("bonus", 0).unwrap();
    assert_eq!(vm.ctx.lookup_scene_no("bonus").unwrap(), 0);
    vm.ctx.reset_active_append_to_initial();
    assert_eq!(vm.ctx.lookup_scene_no("opening").unwrap(), 0);
    assert_eq!(source.config_reads.load(Ordering::Relaxed), 1);
    assert_eq!(
        *source.pack_reads.lock().unwrap(),
        ["base", "extra", "base"]
    );

    use siglus_scene_vm::runtime::forms::{codes::ELM_GLOBAL_G, syscom};
    vm.ctx
        .globals
        .int_lists
        .get_mut(&(ELM_GLOBAL_G as u32))
        .unwrap()[0] = 73;
    vm.restart_scene_name("opening", 0).unwrap();
    vm.ctx.globals.syscom.current_save_scene_title = "Test slot".into();
    vm.ctx.request_auto_savepoint();
    syscom::menu_save_slot(&mut vm.ctx, false, 0);
    vm.step().unwrap();
    vm.step().unwrap();
    syscom::write_config_save(&vm.ctx);
    for key in ["0000.sav", "global.sav", "read.sav", "config.sav"] {
        assert!(saves.read(key).unwrap().is_some());
    }
    drop(host);
    let mut reopened = SiglusHost::new_external_with_boot_source(config, source).unwrap();
    let ctx = &mut reopened.vm_mut().ctx;
    syscom::load_global_save(ctx).unwrap();
    assert_eq!(ctx.globals.int_lists[&(ELM_GLOBAL_G as u32)][0], 73);
    let vm = reopened.vm_mut();
    vm.ctx.set_active_append("extra".into(), "Extra".into());
    vm.restart_scene_name("bonus", 0).unwrap();
    syscom::menu_load_slot(&mut vm.ctx, false, 0);
    vm.step().unwrap();
    vm.step().unwrap();
    assert_eq!(vm.current_scene_name(), Some("opening"));
    assert_eq!(vm.ctx.globals.syscom.current_save_scene_title, "Test slot");
}
