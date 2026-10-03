#!/usr/bin/env python3
"""Compare hand-written 64-bit Unix bindings with a checked-out SDK.

Usage: python3 tools/verify_sdk_abi.py /path/to/vst3sdk
Requires rustc and a C++17 compiler. This does not validate Windows COM ABI.
No SDK files are copied into the repository.
"""
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SDK = pathlib.Path(sys.argv[1]).resolve()


def strip_comments(text):
    return re.sub(r"/\*.*?\*/|//[^\n]*", "", text, flags=re.S)


def snake(name):
    return re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", name)).lower()


def block(text, start):
    depth = 1
    end = start
    while depth:
        depth += (text[end] == "{") - (text[end] == "}")
        end += 1
    return text[start:end - 1]


headers = {}
header_paths = {}
ids = {}
for path in sorted((SDK / "pluginterfaces").rglob("*.h")):
    text = strip_comments(path.read_text())
    for match in re.finditer(r"DECLARE_CLASS_IID\s*\(\s*(\w+)\s*,\s*(0x\w+)\s*,\s*(0x\w+)\s*,\s*(0x\w+)\s*,\s*(0x\w+)\s*\)", text):
        ids[match[1]] = "".join(f"{int(value, 16):08x}" for value in match.groups()[1:])
    for match in re.finditer(r"class\s+(\w+)\s*(?::\s*public\s+(\w+))?\s*\{", text):
        body = block(text, match.end())
        methods = re.findall(r"virtual\s+([\w:*]+)\s+PLUGIN_API\s+(\w+)\s*\((.*?)\)\s*=\s*0", body, re.S)
        if methods:
            headers[match[1]] = (match[2], methods)
            header_paths[match[1]] = path.relative_to(SDK).as_posix()


def methods_for(name):
    parent, methods = headers[name]
    return (methods_for(parent) if parent else []) + methods


aliases = {"ParamValueQueue": "IParamValueQueue", "ParameterChanges": "IParameterChanges",
           "IPlugViewContentScale": "IPlugViewContentScaleSupport", "IConnectionPointProxy": "IConnectionPoint",
           "IEditControllerVtblHeadless": "IEditController"}
field_aliases = {"qi": "query_interface", "ar": "add_ref", "rel": "release",
                 "terminate_component": "terminate"}
pods = {"ProcessSetup", "AudioBusBuffers", "ProcessData", "Chord", "FrameRate", "ProcessContext",
        "NoteOnEvent", "NoteOffEvent", "EventData", "Event", "ParameterInfo", "PFactoryInfo", "PClassInfo",
        "PClassInfo2", "ViewRect"}
base_pods = {"PFactoryInfo", "PClassInfo", "PClassInfo2", "ViewRect"}
cpp_fields = {"channel_buffers_32": "channelBuffers32", "event_type": "type", "data": "noteOn", "continuous_time_samples": "continousTimeSamples"}
includes = ["base/ipluginbase.h", "vst/ivstaudioprocessor.h", "vst/ivstprocesscontext.h",
            "vst/ivstevents.h", "vst/ivsteditcontroller.h", "gui/iplugview.h"]
rust = ["#![allow(dead_code, unused_imports)]", "use std::ffi::c_void; type TUID = [u8; 16];"]
cpp = ['#include <iostream>', '#include <cstddef>'] + [f'#include "pluginterfaces/{name}"' for name in includes]
rust_prints, cpp_prints = [], []
counts = {"ids": 0, "vtables": 0, "signatures": 0, "pod_fields": 0}
signature_prints, signature_expected, signature_includes = [], [], set()
cpp += ['#include <string>', '#include <type_traits>', r"""
template<class T> std::string kind() {
    if constexpr (std::is_void_v<T>) return "v";
    else if constexpr (std::is_pointer_v<T> || std::is_reference_v<T>) return "p";
    else if constexpr (std::is_enum_v<T>) return kind<std::underlying_type_t<T>>();
    else if constexpr (std::is_floating_point_v<T>) return "f" + std::to_string(sizeof(T));
    else return std::string(std::is_signed_v<T> ? "i" : "u") + std::to_string(sizeof(T));
}
template<class R, class C, class... A> std::string signature(R(C::*)(A...)) {
    return kind<R>() + ":" + ((kind<A>() + ",") + ... + std::string{});
}
"""]


def rust_kind(value):
    value = re.sub(r"\b\w+\s*:\s*", "", value.strip())
    if value.startswith("*"):
        return "p"
    value = {"TResult": "i32", "TBool": "u8"}.get(value, value)
    if value in ("", "()"):
        return "v"
    assert value in ("i8", "u8", "i16", "u16", "i32", "u32", "i64", "u64", "f32", "f64"), value
    return value[0] + str(int(value[1:]) // 8)

for path in sorted((ROOT / "src").glob("*.rs")):
    original = path.read_text()
    text = strip_comments(original)
    for match in re.finditer(r"\[\s*((?:0x[\da-fA-F]{2}\s*,?\s*){16})\]", text):
        value = "".join(re.findall(r"0x([\da-fA-F]{2})", match[1])).lower()
        assert value in ids.values(), f"{path.name}: unknown IID {value}"
        counts["ids"] += 1
    structs = {}
    for match in re.finditer(r"(?:struct|union)\s+(\w+)\s*\{", text):
        structs[match[1]] = block(text, match.end())

    def rust_fields(name):
        fields = []
        for field in re.finditer(r"(?:pub(?:\(crate\))?\s+)?(\w+)\s*:\s*(?:unsafe extern \"C\" fn\(.*?\)\s*(?:->\s*[^,]+)?|(\w+))\s*,", structs[name], re.S):
            if field[2] and field[2].endswith("Vtbl"):
                fields += rust_fields(field[2])
            else:
                fields.append(field_aliases.get(field[1].lstrip("_"), field[1].lstrip("_")))
        return fields

    def rust_signatures(name):
        result = []
        for field in re.finditer(r'(?:pub(?:\(crate\))?\s+)?(\w+)\s*:\s*(?:unsafe extern "C" fn\((.*?)\)\s*(?:->\s*([^,]+))?|(\w+))\s*,', structs[name], re.S):
            if field[4]:
                result += rust_signatures(field[4])
            else:
                args = [rust_kind(arg) for arg in field[2].split(',') if arg.strip()][1:]
                result.append(rust_kind(field[3] or "") + ":" + "".join(arg + "," for arg in args))
        return result

    for name in structs:
        if "Vtbl" not in name:
            continue
        interface = aliases.get(name, aliases.get(name.removesuffix("Vtbl"), name.removesuffix("Vtbl")))
        expected = [snake(method[1]) for method in methods_for(interface)]
        actual = rust_fields(name)
        if name == "IEditControllerVtblHeadless":
            expected = expected[:-1]  # Deliberate prefix: headless calls never use createView.
        assert actual == expected, f"{path.name}:{name}\n{actual}\n{expected}"
        counts["vtables"] += 1
        signature_includes.add(header_paths[interface])
        namespace = "Steinberg::"
        if interface in {"IRunLoop", "IEventHandler", "ITimerHandler"}:
            namespace += "Linux::"
        elif interface == "IInfoListener":
            namespace += "Vst::ChannelContext::"
        elif interface not in {"FUnknown", "IPluginFactory", "IPluginFactory2", "IPluginFactory3", "IBStream", "IPlugView", "IPlugFrame", "IPlugViewContentScaleSupport"}:
            namespace += "Vst::"
        for method, abi in zip(methods_for(interface), rust_signatures(name), strict=name != "IEditControllerVtblHeadless"):
            label = f"{path.name}.{name}.{method[1]}"
            signature_prints.append(f'std::cout << "{label} " << signature(&{namespace}{interface}::{method[1]}) << "\\n";')
            signature_expected.append(f"{label} {abi}\n")
            counts["signatures"] += 1

    selected = [(name, body) for name, body in structs.items() if name in pods]
    if not selected:
        continue
    module = path.stem
    rust.append(f"mod {module} {{ use super::*;")
    for name, body in selected:
        kind = "union" if name == "EventData" else "struct"
        alignment = ", align(8)" if kind == "union" else ""
        public_body = re.sub(r"(?m)^\s*(?:pub(?:\(crate\))?\s+)?(\w+)\s*:", r"    pub \1:", body)
        rust.append(f"#[repr(C{alignment})] #[derive(Clone, Copy)] pub {kind} {name} {{{public_body}}}")
        if name == "EventData":
            continue  # Event comparison checks the union's full size and alignment.
        namespace = "Steinberg::" + ("" if name in base_pods else "Vst::")
        cpp_type = namespace + name
        rust_type = module + "::" + name
        label = f"{module}.{name}"
        rust_prints.append(f'println!("{label} {{}} {{}}", std::mem::size_of::<{rust_type}>(), std::mem::align_of::<{rust_type}>());')
        cpp_prints.append(f'std::cout << "{label} " << sizeof({cpp_type}) << " " << alignof({cpp_type}) << "\\n";')
        for field in re.findall(r"(?:pub(?:\(crate\))?\s+)?(\w+)\s*:", body):
            cpp_field = cpp_fields.get(field, field.split("_")[0] + "".join(s.title() for s in field.split("_")[1:]))
            label_field = label + "." + field
            rust_prints.append(f'println!("{label_field} {{}}", std::mem::offset_of!({rust_type}, {field}));')
            cpp_prints.append(f'std::cout << "{label_field} " << offsetof({cpp_type}, {cpp_field}) << "\\n";')
            counts["pod_fields"] += 1
    rust.append("}")

rust.append("fn main() {" + "\n".join(rust_prints) + "}")
cpp += [f'#include "{path}"' for path in sorted(signature_includes)]
cpp.append("int main() {" + "\n".join(cpp_prints + signature_prints) + "}")
with tempfile.TemporaryDirectory(prefix="hostkit-abi-") as directory:
    temp = pathlib.Path(directory)
    (temp / "layout.rs").write_text("\n".join(rust).replace("    pub(crate)", "    pub"))
    (temp / "layout.cpp").write_text("\n".join(cpp))
    subprocess.run(["rustc", "--edition=2024", str(temp / "layout.rs"), "-o", str(temp / "rust-layout")], check=True)
    subprocess.run(["c++", "-std=c++17", "-I", str(SDK), str(temp / "layout.cpp"), "-o", str(temp / "sdk-layout")], check=True)
    actual = subprocess.check_output([str(temp / "rust-layout")])
    actual += "".join(signature_expected).encode()
    expected = subprocess.check_output([str(temp / "sdk-layout")])
    assert actual == expected, "SDK and Rust declarations differ:\n" + "\n".join(f"Rust: {a} | SDK: {b}" for a, b in zip(actual.decode().splitlines(), expected.decode().splitlines(), strict=True) if a != b)
print("PASS:", counts, "64-bit Unix identifiers, vtable order/signature ABI, POD sizes/alignments/offsets")
