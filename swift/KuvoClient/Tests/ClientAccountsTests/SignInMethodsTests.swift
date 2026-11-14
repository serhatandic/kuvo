import ClientAccounts
import Foundation
import OpenCodeKit
import Testing

/// The ways to sign in and a sign-in's status, as both apps read the box's answers: what the box
/// offers (`protocol/vectors/sign-in.json`) comes through whole, in its order, each way
/// one a client can start, with its words and whether it's offered apart.
struct SignInMethodsTests {
    private static func vectors() throws -> [String: Any] {
        let file = URL(filePath: #filePath).deletingLastPathComponent().appending(path: "../../../../protocol/vectors/sign-in.json").standardized
        return try #require(try JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
    }

    @Test func theBoxsWaysInComeThroughInItsOrderWithItsWords() throws {
        let cases = try #require(try Self.vectors()["methods"] as? [[String: Any]])
        #expect(cases.count >= 5)
        for each in cases {
            let offered = try #require(each["offered"] as? [String: Any])
            let methods = try JSONDecoder().decode(SignInMethods.self, from: JSONSerialization.data(withJSONObject: offered))
            let said = try #require(offered["methods"] as? [[String: Any]])
            #expect(methods.methods.map(\.label) == said.map { $0["label"] as? String }, "\(each["provider"]!)")
            #expect(methods.methods.map(\.legacy) == said.map { $0["legacy"] as? Bool ?? false })
            #expect(methods.environment == offered["environment"] as? [String])
            #expect(methods.hint == offered["hint"] as? String)
            // Each one a client starts as the engine lists it: its kind, its id, its questions.
            for (method, box) in zip(methods.methods, said) {
                switch method.method {
                case .oauth(let id, _, _): #expect(box["type"] as? String == "oauth" && box["id"] as? String == id)
                case .external(let id, _, _): #expect(box["type"] as? String == "external" && box["id"] as? String == id)
                case .key: #expect(box["type"] as? String == "key")
                default: Issue.record("\(each["provider"]!): \(method.method) isn't one a client can start")
                }
                #expect(method.method.form.count == (box["form"] as? [Any])?.count ?? 0)
            }
            // Legacy ones come after the others.
            #expect(methods.primary + methods.legacy == methods.methods)
        }
    }

    /// Every account's ways in, by account: what Accounts' Sign In buttons and card hints go by.
    @Test func everyAccountsWaysInAreReadByAccount() throws {
        let cases = try #require(try Self.vectors()["methods"] as? [[String: Any]])
        // The box's answer for a list: each account with its own.
        let listed = Array(cases.prefix(3)).map { each in (each["offered"] as! [String: Any]).merging(["id": each["provider"]!]) { $1 } }
        let read = try JSONDecoder().decode(SignInAccounts.self, from: JSONSerialization.data(withJSONObject: ["accounts": listed + [["id": "nothing", "methods": [Any](), "environment": [Any](), "hint": "Connect"]]]))
        #expect(read.accounts["anthropic"]?.hint == "Claude Pro/Max")
        #expect(read.offersSignIn("anthropic"))
        // One with no way in, and one its box didn't name: no Sign In.
        #expect(!read.offersSignIn("nothing"))
        #expect(!read.offersSignIn("mistral"))
        #expect(SignInAccounts.path(agents: true) == "kuvo/sign-in/accounts?agents=1")
    }

    /// Which accounts Accounts lists, and each one's mark, are the box's word (`listed`, `service`):
    /// as it's recorded for a provider, a web search service, an MCP server's sign-in and the
    /// engine's Claude Agent, signed in and not. No client keeps the rule.
    @Test func whichAccountsAreListedAndTheirMarksAreTheBoxsWord() throws {
        let cases = try #require(try Self.vectors()["accounts"] as? [[String: Any]])
        #expect(cases.count >= 6)
        for each in cases {
            let id = try #require((each["account"] as? [String: Any])?["id"] as? String)
            var said: [String: Any] = ["id": id, "methods": [Any](), "environment": [Any](), "hint": "Connect", "kind": each["kind"]!, "listed": each["listed"]!]
            if let service = each["service"] { said["service"] = service }
            let read = try JSONDecoder().decode(SignInAccounts.self, from: JSONSerialization.data(withJSONObject: ["accounts": [said]]))
            #expect(read.lists(id) == (each["listed"] as? Bool), "\(each["about"]!)")
            #expect(read.services[id] == each["service"] as? String, "\(each["about"]!)")
        }
        // One its box didn't name, or said nothing of listing (none does now: the update gate), isn't listed.
        let bare = try JSONDecoder().decode(SignInAccounts.self, from: Data(#"{"accounts":[{"id":"acme","methods":[],"environment":[],"hint":"Connect"}]}"#.utf8))
        #expect(!bare.lists("acme") && !bare.lists("mistral"))
        #expect(!SignInAccounts.none.lists("anthropic"))
    }

    /// How an account stands and what can be done with it are the box's word (`state`, `line`,
    /// `connections`, `actions`), as it's recorded for each case the box is held to: they come
    /// through whole, a confirm with its words, and nothing is worked out here.
    @Test func anAccountsStateLineAndActionsAreTheBoxsWord() throws {
        let cases = try #require(try Self.vectors()["states"] as? [[String: Any]])
        #expect(cases.count >= 20)
        for each in cases {
            let id = try #require((each["account"] as? [String: Any])?["id"] as? String)
            let offered = try #require(each["offered"] as? [String: Any])
            let answer = offered.merging(["id": id, "kind": "provider", "listed": true, "state": each["state"]!, "line": each["line"]!, "actions": each["actions"]!, "connections": each["connections"]!]) { $1 }
            let read = try JSONDecoder().decode(SignInAccounts.self, from: JSONSerialization.data(withJSONObject: ["accounts": [answer]]))
            let said = try #require(read.said[id], "\(each["about"]!)")
            #expect(said.state.rawValue == each["state"] as? String, "\(each["about"]!)")
            #expect(read.state(id) == said.state)
            #expect(said.line == each["line"] as? String)
            let actions = try #require(each["actions"] as? [[String: Any]])
            #expect(said.actions.map(\.id) == actions.map { $0["id"] as? String }, "\(each["about"]!)")
            #expect(said.actions.map(\.label) == actions.map { $0["label"] as? String })
            #expect(said.actions.map(\.here) == actions.map { $0["here"] as? Bool ?? false })
            for (action, box) in zip(said.actions, actions) {
                let confirm = box["confirm"] as? [String: String]
                #expect(action.confirm?.title == confirm?["title"] && action.confirm?.message == confirm?["message"] && action.confirm?.label == confirm?["label"])
                #expect(said.action(action.id) == action)
            }
            let connections = try #require(each["connections"] as? [[String: Any]])
            #expect(said.connections.map(\.line) == connections.map { $0["line"] as? String })
            #expect(said.connections.map(\.state.rawValue) == connections.map { $0["state"] as? String })
            // Each of the engine's sign-ins finds the box's word for it: by its id, else its place.
            for (index, connection) in connections.enumerated() {
                #expect(said.connection(id: connection["id"] as? String, at: index)?.line == connection["line"] as? String)
            }
            #expect(said.signIn?.id == actions.first { ["signIn", "signInAgain"].contains($0["id"] as? String) }?["id"] as? String)
        }
        // A state or an action a newer box names: not signed in, and the action as it comes, for a
        // client to leave out. An account its box said nothing of has none.
        let newer = try JSONDecoder().decode(SignInAccounts.self, from: Data(#"{"accounts":[{"id":"acme","methods":[],"environment":[],"hint":"Connect","state":"somethingNew","line":"A newer line","actions":[{"id":"somethingNew","label":"A Newer Thing"}],"connections":[]}]}"#.utf8))
        #expect(newer.state("acme") == .signedOut && newer.said["acme"]?.line == "A newer line" && newer.said["acme"]?.actions.map(\.id) == ["somethingNew"])
        #expect(newer.said["acme"]?.signIn == nil)
        #expect(SignInAccounts.none.said["acme"] == nil && SignInAccounts.none.state("acme") == .signedOut)
        #expect(AccountSaid.State.ended.needsYou && AccountSaid.State.problem.needsYou && !AccountSaid.State.signedIn.needsYou)
    }

    @Test func aFailedSignInsWordsComeWithItsStatus() throws {
        let json = #"{"status":"failed","message":"access_denied","failure":"The sign-in was turned down on its page. Try again to sign in."}"#
        let status = try JSONDecoder().decode(SignInStatus.self, from: Data(json.utf8))
        #expect(status.status == .failed)
        #expect(status.failure == "The sign-in was turned down on its page. Try again to sign in.")
        let pending = try JSONDecoder().decode(SignInStatus.self, from: Data(#"{"status":"pending","message":"Downloading… 40%","url":"https://example.com"}"#.utf8))
        #expect(pending.failure == nil && pending.url == "https://example.com")
    }

    @Test func itsRoutesNameTheAccountAndTheAttemptWhole() {
        #expect(SignInMethods.path(integration: "openai", agents: false) == "kuvo/sign-in/methods?integration=openai")
        #expect(SignInMethods.path(integration: "claude-code", agents: true) == "kuvo/sign-in/methods?integration=claude-code&agents=1")
        #expect(SignInStatus.path(integration: "a b", attempt: "att+1/&x", agents: false) == "kuvo/sign-in/status?integration=a%20b&attempt=att%2B1%2F%26x")
        #expect(SignInStatus.path(integration: "codex", attempt: "att_1", agents: true, wait: 20) == "kuvo/sign-in/status?integration=codex&attempt=att_1&agents=1&wait=20")
    }
}
