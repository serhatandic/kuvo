import Foundation

/// Requests and answers for bringing an existing setup into the retained setup.
public enum SetupMerge {
    public struct Pending: Decodable, Sendable {
        public var setup: String
        public var primary: String
        public var operation: String
    }

    public struct Status: Decodable, Sendable {
        public var can: Bool
        public var why: String?
        public var pending: Pending?
        public var completed: Pending?
    }

    public struct Code: Decodable, Sendable {
        public var code: String
        public var expires: Int64
    }

    /// Ask the joining primary whether it can move or has a pending operation.
    public static let status = LinkRequest(path: "kuvo/setup/merge")

    /// Ask any computer in the retained setup for a code for the joining primary.
    public static func code(source: String, computer: String, resume: String? = nil) throws -> LinkRequest {
        var values = ["source": source, "computer": computer]
        values["resume"] = resume
        return try post("kuvo/computers/add/merge-code", values)
    }

    /// Redeem the code on the joining primary and read its accepted merge receipt.
    public static func join(code: String) throws -> LinkRequest {
        try post("kuvo/setup/merge", ["code": code])
    }

    private static func post(_ path: String, _ values: [String: String]) throws -> LinkRequest {
        LinkRequest(method: "POST", path: path, headers: ["content-type": "application/json"], body: try JSONEncoder().encode(values))
    }
}
