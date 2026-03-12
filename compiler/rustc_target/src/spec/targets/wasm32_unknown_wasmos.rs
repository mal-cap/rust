use crate::spec::{Arch, Cc, Env, LinkerFlavor, Os, Target, TargetMetadata, base, cvs};

pub(crate) fn target() -> Target {
    let mut options = base::wasm::options();
    options.os = Os::Other("wasmos".into());
    options.env = Env::Musl;
    options.families = cvs!["wasm"];

    options.add_pre_link_args(
        LinkerFlavor::WasmLld(Cc::No),
        &[
            "--import-memory",
            "--export-memory",
            "--shared-memory",
            "--max-memory=1073741824",
            "--no-entry",
        ],
    );
    options.add_pre_link_args(
        LinkerFlavor::WasmLld(Cc::Yes),
        &[
            "--target=wasm32-unknown-unknown",
            "-Wl,--import-memory",
            "-Wl,--export-memory",
            "-Wl,--shared-memory",
            "-Wl,--max-memory=1073741824",
            "-Wl,--no-entry",
        ],
    );
    options.singlethread = false;
    options.features = "+atomics,+bulk-memory,+mutable-globals".into();

    Target {
        llvm_target: "wasm32-unknown-unknown".into(),
        metadata: TargetMetadata {
            description: Some("WasmOS".into()),
            tier: Some(3),
            host_tools: Some(false),
            std: Some(true),
        },
        pointer_width: 32,
        data_layout: "e-m:e-p:32:32-p10:8:8-p20:8:8-i64:64-i128:128-n32:64-S128-ni:1:10:20"
            .into(),
        arch: Arch::Wasm32,
        options,
    }
}
