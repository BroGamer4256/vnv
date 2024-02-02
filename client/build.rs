fn main() {
	println!("cargo:rerun-if-changed=proto/message.proto");

	prost_build::compile_protos(&["proto/message.proto"], &["proto"]).unwrap();
}
