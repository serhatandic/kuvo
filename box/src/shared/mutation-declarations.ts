// Rules 28, 31 and 34. "Declared owner" resolves through data-declarations.ts.
// An executing computer can enforce its own restrictions even when the primary is away.
export const MUTATION_DECLARATIONS = {
  // Offline edits and forwarded owner writes (settings, notes, labels, queue, wakes and groups).
  ownerWrite: {
    propose: "current submitter", accept: "declared owner", tighten: null,
    perform: null, recover: "declared owner",
  },
  // Messages and drafts. The native backend governs any tools called by a turn.
  queuedSessionWork: {
    propose: "current submitter", accept: "declared owner", tighten: "executing computer",
    perform: "owner's native adapter", recover: "declared owner",
  },
  // Indexed candidates are read-only. Only the declared issuer or its replacement supplies an owner receipt.
  receiptLookup: {
    propose: "current submitter", accept: "declared owner", tighten: null,
    perform: null, recover: "declared owner",
  },
  // A move copies replay fences and forward decisions inside its authenticated scope.
  // Submission outcomes stay at their issuer.
  receiptMove: {
    propose: "authenticated move source", accept: "authenticated move destination", tighten: null,
    perform: null, recover: "move record scope",
  },
  // All published SharedKinds, including membership, assignments and release/security choice.
  sharedSave: {
    propose: "current primary", accept: "current primary", tighten: null,
    perform: null, recover: "declared owner",
  },
  // Live command and shell entry points. Native policies govern any tools started by a command.
  command: {
    propose: "live client or admitted draft", accept: "declared owner", tighten: "executing computer",
    perform: "owner's native adapter", recover: "fresh request",
  },
  // Session ask cards, native permission/form replies and sandbox ask replies.
  consent: {
    propose: "live client", accept: "declared owner", tighten: "executing computer",
    perform: "owner's native adapter", recover: "fresh request",
  },
  // The sandbox tool and automatic review of a current stopped shell call.
  sandboxCommand: {
    propose: "current session tool", accept: "declared owner", tighten: "executing computer",
    perform: "owner's native adapter", recover: "fresh request",
  },
  // Darwin machine keeper handover that requires the active Helper’s Files grant.
  macProtectedAction: {
    propose: "executing computer", accept: "declared owner", tighten: "executing computer",
    perform: "locally OS-granted helper", recover: "fresh request",
  },
} as const;

export type MutationLane = keyof typeof MUTATION_DECLARATIONS;
