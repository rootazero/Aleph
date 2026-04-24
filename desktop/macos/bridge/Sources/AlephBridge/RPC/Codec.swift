import Foundation

/// Line-delimited JSON codec — mirrors the Rust-side
/// `desktop/shared/src/bridge/codec.rs`.
enum Codec {
    static func encode<T: Encodable>(_ msg: T) throws -> Data {
        let enc = JSONEncoder()
        // Deterministic key order helps the golden fixtures in T0.10 stay stable.
        enc.outputFormatting = [.sortedKeys]
        var data = try enc.encode(msg)
        data.append(0x0A) // '\n'
        return data
    }

    static func decode<T: Decodable>(_ line: Data, as _: T.Type) throws -> T {
        try JSONDecoder().decode(T.self, from: line)
    }
}
