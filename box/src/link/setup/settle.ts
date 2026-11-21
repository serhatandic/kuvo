// A member follows the signed setup of a computer that enrolled it over SSH. The
// legacy mac/macs fields also cover this trust when the enroller is a Linux box.
// A member keeps its current primary and epoch rules when it has several legacy enrollers.

import { legacyFolderTrial } from "../../shared/folder-version";
import type { Doc } from "../../shared/documents";
import { IS_MAC, K } from "../home";
import { macDoc, macsOf } from "../peers/known";
import { macSign } from "../state";
import { rankedPrimary } from "../work-queue/numbers";
import { archiveDeviceRevocations } from "./device-revocations";
import { followedSetup, ledSetup, type SetupRecord } from "./record";
import type { LinkState } from "../state";

/** This box's setup brought up to date with the peers documents it holds (in place, under
 * link.json's lock). A machine with no Mac keeps what it leads (`lead.ts` makes it); one a Mac set
 * up gives that up (a setup of no other computer joins its Mac's), and follows the setup its Macs'
 * documents name, the one each names led by the Mac that signed it. */
export function settleSetup(link: LinkState): void {
  // (Within it: a machine with no Mac; one set up by a Mac; the setup it keeps, as its main
  // computer's newest document says it; and one it takes.)
  if (IS_MAC || link.setup?.managed) return;
  const macs = macsOf(link);
  if (!macs.length) {
    if (link.setup?.role === "member") delete link.setup;
    return;
  }
  if (link.setup?.role === "primary") delete link.setup;
  const offers = macs.map(mac => ({ mac, doc: macDoc(link, mac, "peers") })).map(({ mac, doc }) => ({ mac, doc, setup: ledSetup(doc) })).filter(each => each.setup && each.setup.primary === each.mac);
  // (Its main computer's newest document no longer naming it, and another of its Macs leading one:
  // that one, so a machine never stays in none while one of your Macs leads.)
  if (link.setup?.aside && !offers.some(each => each.mac === link.setup!.primary) && offers.length) delete link.setup;
  const kept = link.setup;
  // (The key its main computer signs with, as its setup over SSH last gave it: the trust root.)
  if (kept && macSign(link, kept.primary)) kept.sign = macSign(link, kept.primary)!;
  if (kept) {
    // Its main computer's newest document: the setup as it says it now, or set aside by it.
    const own = offers.find(each => each.mac === kept.primary);
    const doc = macDoc(link, kept.primary, "peers");
    if (own && (own.setup!.id !== kept.id || own.setup!.epoch >= kept.epoch)) {
      // (Its main computer made it again, with its keys as they were and link.json lost: the same main computer.)
      if (own.setup!.id !== kept.id) {
        archiveDeviceRevocations(link);
        link.phones = link.phones?.filter(phone => phone.from !== "mac" || phone.mac && phone.mac !== kept.primary);
      }
      if (own.setup!.id !== kept.id || own.setup!.epoch !== kept.epoch) delete kept.conflict;
      kept.id = own.setup!.id;
      kept.epoch = own.setup!.epoch;
      delete kept.aside;
      if (own.doc) Object.assign(kept, managedFields(own.doc));
    } else if (doc && !ledSetup(doc)) kept.aside = true;
    return;
  }
  // (More than one of its Macs leading a setup, as they may for a moment once a second Mac sets it
  // up: the one it numbers from, the first you set up; none until that's one of them.)
  const ranked = offers.length > 1 ? rankedPrimary(link) : null;
  const taken = offers.length === 1 ? offers[0] : offers.find(each => ranked && "peer" in ranked && ranked.peer.key === each.mac);
  if (taken) link.setup = { id: taken.setup!.id, epoch: taken.setup!.epoch, primary: taken.setup!.primary, sign: taken.doc!.origin, role: "member", ...managedFields(taken.doc!) };
}

/** A peers document this box is given (`takeDoc`), looked at for one more thing: one of its own
 * Macs naming itself the main computer of the setup this box follows, at its epoch, which isn't the
 * main computer this box has. That's two main computers at once (PROTOCOL.md, "Your setup"): this box
 * keeps its own, and notes the other for clients to show. */
export function noteConflict(link: LinkState, doc: Doc): void {
  const setup = followedSetup(link);
  const said = ledSetup(doc);
  if (setup?.role !== "member" || !said || said.id !== setup.id || said.epoch !== setup.epoch || said.primary === setup.primary) return;
  if (!macsOf(link).includes(said.primary) || macSign(link, said.primary) !== doc.origin) return;
  setup.conflict = { primary: said.primary, epoch: said.epoch };
}

export function managedFields(doc: Doc): Pick<SetupRecord, "managed" | "checkpointed" | "leadershipHash"> {
  if (doc.body.managed !== true || legacyFolderTrial(K)) return {};
  return { managed: true, ...(doc.body.checkpointed === true ? { checkpointed: true as const } : {}),
    ...(doc.body.checkpointed === true && typeof doc.body.leadershipHash === "string" &&
      /^[\w-]{11}$/.test(doc.body.leadershipHash) ? { leadershipHash: doc.body.leadershipHash } : {}) };
}
