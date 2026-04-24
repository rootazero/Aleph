import Foundation

// MARK: - Codable ↔ JSONValue helpers
//
// These two free functions let handlers encode typed Codable structs into
// JSONValue and decode JSONValue params into typed structs without hand-rolling
// every field.  They work because JSONValue itself is Codable: the round-trip
// through Data is essentially free for small payloads.
//
// Usage in handlers:
//   let result = try encodeCodable(QueryResult(element: el))
//   let args   = try decodeCodable(params, as: QueryTreeParams.self)

/// Encode any `Encodable` value into a `JSONValue`.
///
/// Internally encodes to JSON `Data` then decodes as `JSONValue`, so the
/// result faithfully represents the original value's JSON serialization.
func encodeCodable<T: Encodable>(_ value: T) throws -> JSONValue {
    let data = try JSONEncoder().encode(value)
    return try JSONDecoder().decode(JSONValue.self, from: data)
}

/// Decode a `JSONValue?` params payload into any `Decodable` type.
///
/// A `nil` params is treated as JSON `null`, which lets structs with all-optional
/// fields decode gracefully from missing params.
func decodeCodable<T: Decodable>(_ params: JSONValue?, as type: T.Type) throws -> T {
    let data = try JSONEncoder().encode(params ?? .null)
    return try JSONDecoder().decode(type, from: data)
}
