import { keepEngineOnDemand } from "./engine/lifetime";
import { updateUnderWay } from "../shared/updating";
import { locked, writeSecret } from "../shared/files";
import { once } from "node:events";
import { readServices } from "../shared/box-services";
import { processRuns } from "../shared/platform";
import { isObject, parsed } from "../shared/json";
import { LABEL_RESTORE_RESULT, LABEL_RESTORE_ROUTE } from "../shared/routes/work-queue";
import { fitsShape, type Infer } from "../shared/shape";
import { ledSetup, usesAcceptedSaves } from "./setup/record";
// (The command line: what each command does is in the modules beside it, box/README.md.)
// Kuvo Code's link on a machine: lets the phones you've paired reach this machine's engine
// through the relay, so its sessions and bots work from the phone with the Mac closed. The
// engine's own runtime runs it, so the machine needs nothing else:
//
//   BUN_BE_BUN=1 opencode kuvo-link.js run             the bridge, started beside the engine, which
//                                                      also tells your phones what happens in the
//                                                      machine's sessions while no Mac does
//   BUN_BE_BUN=1 opencode kuvo-link.js setup           the Mac's phones and relay, as JSON on stdin
//   BUN_BE_BUN=1 opencode kuvo-link.js pair [relay]    a link to pair a phone with this machine
//   BUN_BE_BUN=1 opencode kuvo-link.js selftest        checks the runtime has what it needs
//
// It keeps its state in ~/.kuvo-code/run/link.json (mode 600), reads the engine's port and
// password from run/engine.json, and connects out to the relay; it never listens. The protocol
// is documented in protocol/PROTOCOL.md.

import { LEADERSHIP_WAIT, primaryFenced } from "./setup/lease";
import { ROUTE_VERSION } from "../shared/route-version";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../shared/format-version";
import { unsupportedFormatReply } from "./server/format";
import { isSetupReservation, settingsSeedFormat, setupInputFormat, UNSUPPORTED_RESERVATION } from "./add/command-formats";
import { takeSeed } from "./add/seed";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { plainName } from "../shared/text";
import { sleep } from "../shared/wait";
import { HangWatch, isBridge as runsBridge } from "../shared/hang";
import { joinPath, loginEnvironment, loginPath, saveLoginPath, savedLoginPath } from "../shared/platform";
import { launchFromLogin } from "./run-command";
import { watchAsks } from "./areas/asks";
import { watchWorkAsks } from "./work/asks";
import { setBoxKey } from "./areas/box-keys";
import { addMacDefaults, changeSettings, ownsSettings, readSettings } from "./areas/config";
import { engineConnect } from "./areas/engine";
import { reconnectEverywhere, upgradeBeforeEngine, upgradeIntegrations } from "./areas/integrations";
import { readApart } from "./github/graphql";
import "./work-queue/github";
import "./glance/github";
import { keepMarketplaceProjection } from "./areas/marketplace";
import { startMarketplaceRelay } from "./marketplace/connections";
import { keepSkillTrashClear } from "./areas/skills";
import { sendWakes } from "./areas/wakes";
import { finishDeletes } from "./bots/edit";
import { dropBotsOwnKeys, dropBotsOwnServers, keepBotsOwnKeysGone } from "./bots/own-keys";
import { keepJobs } from "./bots/jobs";
import { forgetOldKit, readKit } from "./bots/kit";
import { keepZone, zoneFollowers } from "./zone/setting";
import { adoptBox } from "./compat/adopt-settings";
import { forgetPhones } from "./compat/leave-running";
import { addressOf, newKey, publicOf } from "./crypto/keys";
import { IKInitiator, IKResponder, openOneWay, PROLOGUE, sealOneWay } from "./crypto/noise";
import { docsNoted, noteDocsChanged, onDocsChanged } from "./documents/changed";
import { sendSetupCopies } from "./setup/seed";
import { docsChanged, macDocsChanged } from "./documents/exchange";
import { authorizeDeviceConnection, refreshPhoneTrust } from "./documents/phone-sync";
import { newSigningSeed, ownSign, readDoc, SIGN } from "./documents/format";
import { docsWork, ownDocs, pruneDocs } from "./documents/work";
import { bridgeTarget, peerPlaces, peerTargets, readBridgeFile, readEngineFile } from "./engine/client";
import { applyCompatibility, engineConfigContent, knowsEngineSettings } from "./engine/compatibility";
import { keepEngineDefaults } from "./engine/defaults";
import { clearNotesLocks } from "./engine/notes";
import { watchBox } from "./engine/watch";
import { watchMoves } from "./moves/move";
import { writePrivate } from "./files";
import { browserStatus, installBrowser, startBrowserInstall } from "./browser/install";
import { foldHistory } from "./browser/history";
import { takeInAppBrowserData, thisMacsAppFolder } from "./browser/from-app";
import { APP_FOLDER, BRIDGE_FILE, IS_MAC, K, KEYS_ON_HOST, MAC_APP_VERSION, NEEDED_FILE, OWN_HOME_MAC, OWN_RUNTIME, PID_FILE, REAL_BOX, RUN, scriptIs, socketKey, takeVaultSecrets, upgradeLinkFolder, VERSION_FILE } from "./home";
import { activeInstallerClaim, INSTALLER_RECOVERY_ROUTE } from "./add/invite-state";
import { enrollInstalled, installCommand, installerCode } from "./install/command";
import { archiveInstallerCancellation, installerCancellationReceipts } from "./add/invite";
import { rehomeCommand } from "./rehome/command";
import { clearKept } from "./rehome/move";
import { readServed, servedFormat, SERVED_FILE } from "./rehome/served";
import { settleOwedAtStart } from "./rehome/route";
import { isBridge, keepKeeper, machineServerStartLock, needed, noLongerNeeded, relayNeeded, residentRunning, runningBridge, startBridge } from "./keeper";
import { noteRelay } from "./letters/leave";
import { deliverLeftAnswers, outboxSoon, sendOutbox } from "./letters/outbox";
import { SENDING } from "./letters/letter";
import { watchFolders } from "./follow";
import { pushActivity } from "./work/activity-peers";
import { localActivity } from "./work/activity";
import { pushRows } from "./work/elsewhere";
import { pushNumbers } from "./work-queue/numbers";
import { archiveDeviceRevocations, takesPhoneScope } from "./setup/device-revocations";
import { hasSetupWork } from "./setup/existing-work";
import { ensurePrimaryLease } from "./setup/relay-lease";
import { startTakeover } from "./setup/takeover";
import { initializeMachineSetup, noteSetupNow, settleOwnSetup } from "./setup/lead";
import { PRIMARY_UNLINK } from "./setup/leadership";
import { macName, mainElsewhere, pairElsewhere } from "./setup/main-only";
import { writeCard } from "./setup/card";
import { hereNow } from "./setup/judge";
import { shareArchived } from "./areas/settings-pages";
import { shareOutputs } from "./areas/setup-outputs";
import { shareNotices } from "./areas/notices";
import { shareLibrary } from "./areas/library";
import { shareSetupLists } from "./lists/setup";
import { listFollowers } from "./lists/sessions";
import { pushQueues } from "./work-queue/across";
import { FORWARDS, forwardsSoon, sendForwards } from "./work-queue/forward";
import { reachAgainSoon, sendReachAgain } from "./peers/reach-again";
import { heldSoon, sendHeld } from "./bots/held";
import { handUpkeepSoon } from "./work-queue/hand";
import { clearStaging } from "./work-queue/ops";
import { macLinkCheck, tellUntold } from "./mac/link";
import { macNoticeHold, macTell } from "./mac/notices";
import { runMacPeers } from "./mac/peers";
import { isDiskWorker } from "./server/disk-worker";
import { isReceiptWorker, refreshReceipts } from "./ownership/receipt-worker";
import { macCompatibilityFront, runMacLink } from "./mac/relay";
import { watchVaultAsks } from "./vault/asks";
import { watchSandboxAsks } from "./fence/outside";
import { askHolder, holdVault } from "./vault/holder";
import { primeGenerations } from "./vault/store";
import { setVaultTell } from "./vault/use";
import { HELD_FILE, NoticeHold } from "./notices/hold";
import { keepTold, markTeller, watchSentByAgents } from "./notices/told";
import { sendToPhones } from "./notices/push";
import { announce, forgetMacsAgents, refreshOwn } from "./peers/agents";
import { forgetOldMacs, KEY, leaveMac, macsOf, noteOnline, ownKey, peersOf, takeLegacyPeers } from "./peers/known";
import { peerMail, receiveMail } from "./peers/mail";
import { tellSeatPeers, watchSeats, whereChanged } from "./peers/seats";
import { savePersonalityLibrary } from "./areas/personalities";
import { changePersonalities, mergePersonalities, readPersonalities } from "./personalities/library";
import { takePersonalities } from "./personalities/take";
import { confirmPhone, currentPhonePairings, deviceConnectionsAllowed, foldPhones, newPairingToken, PAIRING_MINUTES, pairingLink } from "./phones";
import { connectOnce, nextTry } from "./relay/connection";
import { LinkHost } from "./relay/host";
import { createBoxDispatcher } from "./server/router";
import { serveBox } from "./server/serve";
import { liveLink, machineLink, macLink, macSign, onLinkWritten, readLink, updateLink } from "./state";
import { keepKeyFiles } from "./keys";
import { keepAwakeWhileWorking } from "./work/awake";
import { readServeRecord, runningServe, SERVE_FILE, SERVE_NEEDED, writeServeRecord } from "./serve-kept";
import { scheduleWipe, wipeCommand } from "./setup/wipe";
import { sendRemovalUpdates } from "./setup/removal";
import { logCleanup, uninstallCommand } from "./uninstall";
import { helperCommand } from "./helper";
import { handOverCommand, watchHelperBuild } from "./compat/helper-handover";
import { keepMachinesUp } from "./add/keep-up";
import { automaticSshUpdate } from "./areas/computers-add";
import { leaveForNewCode } from "./updates/code-in-place";
import { sayWhole } from "./once";
import { liveResidentInfo, settleUpdate } from "./updates/health";
import { clientServerStopped, ownClientServer, residentInstalled, type ClientOwner } from "./server/resident";
import { handOverMachine, recoverMachineJob, startMachineInstall, swapEngine } from "./updates/engine-swap";
import { activeBin, convertBin } from "./updates/versions";
import { continueCutTurns } from "./updates/interrupted";
import { boxVersion, installedPack, takeUpdateLock, UPDATE_LOCK, UPDATING_ALREADY, agentsWorking, dropOldBackups, updateOnTrial, isUpdating, requestUpdateNow, noteUpdate, update, updateStatus, versionOrder } from "./updates/update";
import { checkFolderVersion, KUVO_FOLDER_LAYOUT, setupLayoutDeferred, unlinkCleanupLayoutReady } from "../shared/folder-version";
import { answerSshRoute, writeSshBytes, type SshCommand } from "./server/ssh-route";
import { pipeSshRoute, selectSshPipe, serveSshPipe, sshPipeSelected } from "./server/ssh-pipe";
import { BRIDGE_VERSION } from "./version";
import { ADD_RESERVATION, SETUP_LOCAL_PAIRINGS, SETUP_OCCUPIED } from "./marks";
import type { AddressInfo } from "node:net";
import type { SignedDoc } from "../shared/documents";
import type { EngineRecord } from "../shared/records";
import { fenceOwnPort, keepFence } from "./fence/state";
import { keepApprovals } from "./areas/approval";
import { keepGroupSettings } from "./areas/group-settings";
import { keepOldBranchPrefix } from "./areas/git-words";
import type { KeptNotice } from "./notices/hold";
import type { Notice } from "./notices/notice";
import type { GivenLibrary } from "./personalities/library";
import type { LinkHandler } from "./server/context";
import type { LinkState } from "./state";

// What moved into modules, as this file exported it (the bridge's tests import it from here).
export { addressOf, newKey, publicOf } from "./crypto/keys";
export { CipherState, IKInitiator, IKResponder, MAIL_PROLOGUE, openOneWay, PROLOGUE, sealOneWay } from "./crypto/noise";
export { decodeMessage, encodeMessage, Frame, frame, pieces, readFrame, readRelay, relayHello, relayPush, relaySend } from "./relay/frames";
export { isSecretName, markingHashes, masked, maskedHexValues, maskedNamed, namesHash, showsAll } from "./notices/mask";
import type { CommittedReceipt, Receipt, SaveEnvelope } from "../shared/routes/setup-save";
export { askSetupOutcome, askSetupSave, askSetupStatus } from "../shared/setup-local";
export { docBytes, MOST_DOC, newSigningSeed, ownSign, readDoc, signDoc, signingKeyOf } from "./documents/format";
export { IS_MAC, LINK_FILE } from "./home";
export { boxPlaces } from "./places";
export { BRIDGE_VERSION } from "./version";
export { FEATURES } from "../shared/features";
export { macFeatures } from "../shared/routes/mac";
export { messageID } from "./engine/sessions";
export { forgetMovesInMemory, keepMoving, sendMovesWith, takeMoveMail, watchMoves } from "./moves/move";
export { movesElsewhere } from "./moves/elsewhere";
export { applyCompatibility, COMPATIBILITY_FILE, ENGINE_LAYER_FILE, engineConfigContent, learnCompatibility, readCompatibility } from "./engine/compatibility";
export { MODELS_DUE_FILE, refreshModels, settleDueModels } from "./engine/models";
export { queueOf, queueWatcher, readQueues, scanQueues, stopSession } from "./engine/queue";
export { PLACE_TRIES } from "./letters/letter";
export { forgetEverythingSent, SENDING_PATIENCE } from "./server/sent-once";
export { keepReadThroughSummary, keepWorkRead, settleWorkRead, under } from "./work/tree";
export { readAllOn, readGroups } from "./areas/read-all";
export { glances, Unreached } from "./areas/glance";
export { forgetModelLists, forgetModelNames, noteModelsEvent } from "./work/model-rule";
export { askedSince, forgetAsks, forgetAsksOn, noteAskEvent } from "./glance/asked";
export { checksOf, pullOf, queryFor, readPulls } from "./glance/github";
export { linkPull, pullCatcher, readPullLinks } from "./glance/pulls";
export { letAttachmentsGo } from "./server/attached";
export { FIRST_LOOK, WAITING_LOOK, workWatcher } from "./work/wakes";
export { forgetOldHomes } from "./work/homes";
export { AT_WORK_LIMIT, atWorkNow, noteStopped, readBackground, readStopped, stopWork } from "./work/background";
export { awakeForWork, holdForWork, keptAwake, letGo, lookAtWork, UNTOLD_FOR, WORK_AWAKE_FILE } from "./work/awake";
export { macLink, readLink, updateLink, writeLink } from "./state";
export { searchPlugins } from "./areas/plugins";
export { listFolders, makeFolder } from "./areas/folders";
export { findProgram } from "./areas/programs";
export { providerModels } from "./areas/providers";
export { endedProviders, forgetPlan, noteSignIn } from "./areas/accounts";
export { answerAsk, askAnswer, askNotice, listedAsk, readAsks, watchAsks } from "./areas/asks";
export { BOT_MEMORY_RULE, addMacDefaults, changeSettings, deepMerged, keysNamedHere, ownsSettings, readSettings } from "./areas/config";
export { addFromCatalog, mcpCatalog } from "./areas/mcp-catalog";
export { adoptBox } from "./compat/adopt-settings";
export { setBoxKey } from "./areas/box-keys";
export { applySettings, translateSettings } from "./areas/apply";
export { NOTICE_LIMIT, noticeJSON, pushTo } from "./notices/notice";
export { notePushed, PUSHED_FILE, readPushed, sendToPhones } from "./notices/push";
export { LinkHost } from "./relay/host";
export { followed } from "./follow";
export { answerIntegrations, integrationCatalog, INTEGRATIONS_FILE, reconnectEverywhere, upgradeIntegrations } from "./areas/integrations";
export { allWakes, holdArchived, listWakes, nextWake, readWakes, repeatText, sendWakes, wakeFolder, wakeOf } from "./areas/wakes";
export { readRemovedWorktrees, readWorktrees } from "./areas/worktree-records";
export { answerProjects, boxProjects, changeProjects, readProjects } from "./areas/projects";
export { answerWorktrees, branchSlug, nameWorktreeBranch, tellWorktree, tidyWorktrees, worktreesWhere } from "./areas/worktrees";
export { SKILL_REPOSITORIES, answerSkills, keepSkillTrashClear, skillAbout, skillName, skillTrashEntry, skillsIn } from "./areas/skills";
export { agentsAllowed, allowed, canonicalPath, signInReturn, socketAllowed } from "./server/paths";
export { KEEP_FOR, SEEN_FILE, forgetPhones, phoneSeenLately } from "./compat/leave-running";
export { engineConnect, engineControl, toolRun } from "./areas/engine";
export { botPersonalities, changePersonalities, mergePersonalities, personalitiesDocument, readPersonalities } from "./personalities/library";
export { keyFilesWait, syncKeyFiles } from "./keys";
export { KEEP_UP_FILE, keepUpRound, keepingSelfUp, keepingUp } from "./add/keep-up";
export { codeCheck, codeInPlace } from "./updates/code-in-place";
export { HEALTH, lookAtPieces, rollBackCode, settleUpdate } from "./updates/health";
export { appBehind, registerDesktop } from "./mac/app";
export { noteTrialEnded } from "./updates/update";
export { consumeFleetRetry, fleetSummary, peerFleetUpdate, localFleetUpdate, requestFleetRetry } from "./updates/progress";
export { requestedRetry } from "./updates/attempt";
export { askMainToCheck, consumeFleetCheck, fleetFacts, fleetWith, followPeers, forgetFleet, heldRelease, keepRelease, newestHeld, pullUnderWay, takeReleaseMail, watchHeld } from "./updates/fleet";
export { UPDATE_KEYS, agentsWorking, boxVersion, dropOldBackups, checkDrift, confirmUpdate, isUpdating, newerVersion, noteLauncher, receiveRelease, releaseRound, requestUpdateNow, startUpdate, update, updateStatus, updatesBlocked, verifyBoxes, versionOrder } from "./updates/update";
export { activationAllowed, fleetTarget, STAGE_WAIT } from "./updates/fleet";
export { keepLegacyRelease } from "./updates/keep";
export { publishedShared } from "./setup/shared";
export { FILE_LIMIT } from "./areas/file";
export { firstMessageOf, jobInMemory, jobLine, keepJobs } from "./bots/jobs";
export { listBots } from "./bots/list";
export { botInstructions, botTemplates, readKit, readWords } from "./bots/kit";
export { applyZone, readZoneSetting, setZone, takeZone, zoneHere, zoneMail } from "./zone/setting";
export { freshenBot, freshenBots, refreshedRules, settleBot } from "./bots/tell";
export { makeBot } from "./bots/make";
export { carryToAgents } from "./areas/carry";
export { changeBot, deleteBot, finishDeletes } from "./bots/edit";
export { dropBotsOwnKeys } from "./bots/own-keys";
export { ASK_HOLD, LONGEST_LEASE, NEWS_HOLD, NoticeHold, RESTORED_WAIT, SHOWN_WITHIN, attentionGiven, holdFor } from "./notices/hold";
export { alwaysTitle, describeForm, describePermission, groupNotice, noticeFor, projectNameOf, settleHeld } from "./notices/news";
export { STEER_AFTER, steering, watchBox, watchEvents } from "./engine/watch";
export { forgetOldMacs, leaveMac, macsOf, mailFor, movedOf, noteOnline, openMail, peersOf, takeLegacyPeers } from "./peers/known";
export { noteDocsChanged } from "./documents/changed";
export { authorizePhone, confirmPhone, foldPhones, newPairing, ownPhones, pairDevice, pairPhone, removeAsMac, removePhones } from "./phones";
export { deviceAct, givenDevice, pairDeviceFromHere, pairHere } from "./peers/devices";
export { noLongerNeeded } from "./keeper";
export { changedWork, digestOf, docMails, docsWork, greetWork, primaryPeersBody, primaryPeersBody as macPeersBody, ownDoc, takeDoc } from "./documents/work";
export { leadsNow, noteSetupNow, settleOwnSetup } from "./setup/lead";
export { settleSetup } from "./setup/settle";
export { followedSetup, newSetupID, startsFromOthers } from "./setup/record";
export { setupView } from "./setup/view";
export { foundOf, hereNow, judgeComputer, judgeHere, oursNow, OWN_BOX_RAN } from "./setup/judge";
export { cardNow, SETUP_CARD, writeCard } from "./setup/card";
export { keptComputer, keptFeatures, peerOf, readPeerAgents, takeAgents } from "./peers/directory";
export { actionsOf, oldAppAttached, removeHere, renameHere, writerAct, writerOf } from "./peers/manage";
export { canAdd, otherAddressesOf } from "./add/flow";
export { ownAddresses } from "./areas/computers";
export { askReachAgain, readTries, sendReachAgain, TRY_FOR } from "./peers/reach-again";
export { listComputers, ownComputer } from "./areas/computers";
export { composeLetter } from "./letters/letter";
export { composeAgents } from "./peers/directory";
export { nameAgents, namedAgents, namesIn } from "./peers/naming";
export { bridgeAsk, bridgeClock, carrySeat, seatPayloads, seatPeers, takeLimits, takeSeat, tellSeatPeers, watchSeats, whereChanged } from "./peers/seats";
export { announce, ownAgents, refreshOwn, writeAgents } from "./peers/agents";
export { answered, deliverLeftAnswers, letterMail, letterWent, sendOutbox } from "./letters/outbox";
export { takeLetter } from "./letters/take";
export { answerResumed, callAs, callsBack, sendCall, takeCall } from "./letters/calls";
export { computerOf, resumeStarts, stopBotWorkElsewhere } from "./letters/start-call";
export { agentTool } from "./letters/tools";
export { stopBotWork } from "./work/stop";
export { allowOtherComputers, approvalOf, otherComputersAllowed } from "./work/approval";
export { reapplyApprovals } from "./areas/approval";
export { pushRows, takeRows } from "./work/elsewhere";
export { askToSend, blockArrived, BLOCK as QUEUE_BLOCK, CLAIM_REACH as QUEUE_CLAIM_REACH, claimNumbers, giveBlock, LOW as QUEUE_LOW, notePrimaryNow, primaryOf, pushNumbers, rankedPrimary, readNumbers, reclaimWaiting, takeBlock, takeNumber, takeNumbersMail } from "./work-queue/numbers";
export { forgetPushed as forgetQueuesTold, itemsElsewhere, pushQueues, takeWorkQueueMail, tellingNow as queueTellingNow, tellQueuesAgain } from "./work-queue/across";
export { handUpkeep } from "./work-queue/hand";
export { forgetHeldSends, HELD as BOTS_HELD, heldLists, heldSoon, holdEdit, keeperOf, sendHeld, takeBotEdit, takeBotEdited } from "./bots/held";
export { forgetActs as forgetQueueActs, FORWARDS as QUEUE_FORWARDS, forwardsSoon as queueForwardsSoon, forwardTiming as queueForwardTiming, sendForwards as sendQueueForwards, takeAct as takeQueueAct } from "./work-queue/forward";
export { actForPeer as queueActForPeer } from "./work-queue/acts";
export { mailsFor as queueMailsFor, takePart as takeQueuePart } from "./work-queue/parts";
export { parseBacklog } from "./work-queue/import";
export { carryWorkQueue, forgetWorkQueues, packWorkQueues, placeWorkQueues } from "./work-queue/moves";
export { forgetMigrated as forgetQueueMigrated } from "./work-queue/migrate";
export { atWorkForSure as queueAtWorkForSure, forgetSettled as forgetQueueSettled, settleStarted as settleQueueStarted } from "./work-queue/started";
import { atWorkForSure, settleStarted } from "./work-queue/started";
import { checkRepairFormat, repairWithFetch } from "./updates/repair";
import { checkRepairRequest, repairReplyFormat } from "./updates/repair-record";
import { readSwap, installHoldFile } from "../shared/engine-swap";
import { checkRollbackRecords, finishReleaseHold, releaseSwitchQuiet } from "./updates/update";
import { readHandOver } from "./compat/helper-handover";
import { readMachineJob } from "../shared/machine-job";
import { fleetWith } from "./updates/fleet";
import { keepFleetUpdates } from "./updates/keep";
import { installedWslTask, keepWindowsAwake, startWslTask } from "./windows";
import { commandsWorking } from "./run-command";
import { nextWake } from "./areas/wakes";
export { readReach } from "./peers/reach";
export { peerPlaces } from "./engine/client";
export { answerPersonalities, retellBots, takePersonalities, takePersonalitiesDocument } from "./personalities/take";
export { docsChanged, macDocsChanged, receiveDocs } from "./documents/exchange";
export { peerMail, receiveMail } from "./peers/mail";
export { pairingLink } from "./phones";
export { ownRefreshed } from "./server/naming";
export { FROM_PHONE, fromPhone } from "./server/forward";
export { DEFAULT_RELAY, MAC_LINK_FILE, macKeys, ownMacConfig, savedMacConfig, stopMacLink } from "./mac/link";
export { macNoticeHold, macTell } from "./mac/notices";
export { LETTER_WAIT, macSendOutbox, runMacPeers, stopMacPeers } from "./mac/peers";
export { setMacPeers } from "./mac/routes";
// Adding a computer over SSH (the bridge's tests').
export { allTries, forgetTries } from "./add/tries";
export { renewsItself, seedCredentials, seedSettings } from "./add/seed";
export { adding } from "./areas/computers-add";
export { KNOWN_HOSTS, fingerprintOf } from "./add/ssh";
export { RELEASE_FILES } from "./add/files";
export { engineHandler } from "./server/router";
export { connectOnce, fromRelay, nextTry } from "./relay/connection";
export { MAC_APP_ROUTES, macAppAnswers, macFront, runMacLink } from "./mac/relay";
export { forgetLoginItems } from "./mac/own-mac";
export { keepKeeper } from "./keeper";
export { serveBox } from "./server/serve";

// This file, for what the box runs again as a command of its own.
scriptIs(fileURLToPath(import.meta.url));

// ---- What follows a change to link.json (./state), or to this box's documents ----

onDocsChanged(macDocsChanged);

// Your time zone changed here (a pick, a peer's, an older Mac's kit): your other computers hear at once.
zoneFollowers.add(() => {
  const host = liveLink();
  if (host?.welcomed) announce(host, readEngineFile).catch(() => {});
});

onLinkWritten(link => {
  // The setup it follows changed: its bridge and its peers hear at once (the usage panel's
  // settings go between the boxes of one setup).
  if (whereChanged(link)) {
    tellSeatPeers(link, true).catch(() => {});
    const host = liveLink();
    if (host?.welcomed) announce(host, readEngineFile, link).catch(() => {});
  }
  if (IS_MAC) {
    for (const tell of macLink.phoneFollowers) tell();
    // Phones no longer paired lose what they have open, whoever removed them, and a link no longer
    // wanted (the last phone gone) leaves the relay: a moment after, so a phone that removed itself
    // gets its answer first (as a machine's bridge does). Known: closing once that answer has gone
    // would be surer than a moment's wait.
    setTimeout(() => {
      macLink.host?.keepOnly(currentPhonePairings(readLink()));
      macLinkCheck();
    }, 200);
    return;
  }
  // Devices no longer let in (removed on another computer, as its phones document says) lose what
  // they have open, a moment after, as on a Mac.
  if (machineLink.host) setTimeout(() => machineLink.host?.keepOnly(currentPhonePairings(readLink())), 200).unref?.();
  // For the box's keeper, which keeps this bridge running while it's needed.
  if (needed(link)) fs.writeFileSync(NEEDED_FILE, "", { mode: 0o600 });
  else fs.rmSync(NEEDED_FILE, { force: true });
});

// ---- The commands ----

// What a machine's link does once the relay's welcomed it: your other computers hear who's here,
// and it tells who's there.
function machineWelcomed(host: LinkHost): void {
  try { refreshPhoneTrust(host); } catch {}
  announce(host, readEngineFile, undefined, undefined, true).catch(() => {});
  try { docsChanged(host, { greet: true }); tellUntold(); sendRemovalUpdates(host); } catch {}
}

/** This box's engine while it runs, for what's put right in a conversation open there. */
const runningEngine = (): EngineRecord | null => { try { return readEngineFile(); } catch { return null; } };

// An update judged well (updates/health.ts) is here to stay: what waited for that goes on. (Where
// this Mac's helper keeps its keys, its server, started again on the new code, does that.)
async function updateIsWell(): Promise<void> {
  await upgradeIntegrations().catch(() => {});
  if (!KEYS_ON_HOST) await dropBotsOwnKeys(runningEngine).catch(() => {});
}

async function run(owner?: ClientOwner): Promise<void> {
  const resident = !!owner;
  const startedAt = Date.now();
  const report = (event: string, fields: Record<string, unknown> = {}) =>
    console.error(JSON.stringify({ at: new Date().toISOString(), pid: process.pid, event, ...fields }));
  const old = locked(machineServerStartLock, () => {
    let pid = 0;
    try { pid = Number(fs.readFileSync(PID_FILE, "utf8")); } catch {}
    if (pid !== process.pid && residentRunning(pid)) {
      if (resident) throw new Error("The resident box already runs.");
      return null;
    }
    const serving = resident ? runningServe({ whenUnknown: true }) : null;
    if (resident && (isBridge(pid) || serving && serving.pid !== process.pid)) throw new Error("Stop the existing box client processes before starting resident serve.");
    // The legacy link can still replace another legacy link.
    if (isBridge(pid)) try { process.kill(pid, "SIGTERM"); } catch {}
    if (resident) selectSshPipe();
    writePrivate(PID_FILE, String(process.pid));
    writePrivate(VERSION_FILE, BRIDGE_VERSION);
    return pid;
  }, { exclusive: true });
  if (old === null) return;
  if (resident) keepEngineOnDemand();
  const stopServices = resident ? keepResidentServices() : undefined;
  let stopWindowsAwake: (() => void) | undefined;
  let leaving = false;
  let directoryChanged = false;
  const noteDirectory = () => { directoryChanged = true; };
  listFollowers.add(noteDirectory);
  const cleanUp = async (reason: string) => {
    if (leaving) return;
    leaving = true;
    stopWindowsAwake?.();
    listFollowers.delete(noteDirectory);
    report("link stopping", { reason });
    if (owner) {
      await owner.drain();
      stopServices?.();
      removeOwnServeRecord(owner.generation);
    }
    removeOwnLinkRecord();
    process.exit(0);
  };
  process.on("SIGTERM", () => cleanUp("SIGTERM"));
  process.on("SIGINT", () => cleanUp("SIGINT"));
  // New code put in place (an update): the supervisor can start it again on it.
  leaveForNewCode(fileURLToPath(import.meta.url), () => cleanUp("bundle changed"));
  report("link starting", { version: BRIDGE_VERSION });
  (await import("./backup/engine")).keepDailyBackups();
  if (resident) initializeMachineSetup();
  // (Sessions' turns' lock folders left by a box cut off: `engine/notes.ts`.)
  clearNotesLocks();
  // The programs it looks for (an MCP server's `npx`) found where the engine finds them: on the
  // PATH your login shell sets up, as the engine's start read it, which the keeper's start doesn't get.
  const saved = savedLoginPath(K);
  if (saved) process.env.PATH = joinPath(path.join(K, "bin"), saved);
  // COMPAT(beta.1 connected): `run/connected` (beta.1) and `run/present` (0.7 with the phone link on), cleared. Goes when 0.8.0 ships.
  // The notes an earlier Kuvo Code on a Mac left that it was connected (it did some of the box's
  // work then): nothing reads them now.
  for (const folder of ["present", "connected"]) fs.rmSync(path.join(RUN, folder), { recursive: true, force: true });
  // Its bots' jobs kept in their folders, taken once from where they were before.
  try { keepJobs(); } catch {}
  forgetOldKit();
  // Its integrations and servers as its catalog has them now, and what's left of its bots' own keys
  // gone (bots/own-keys.ts).
  await upgradeIntegrations().catch(() => {});
  await dropBotsOwnKeys(runningEngine).catch(() => {});
  // A link from an earlier bridge: its signing key made, and kept as this one keeps it.
  updateLink(link => {
    if (!link) return null;
    link.sign ??= newSigningSeed();
    settleOwnSetup(link);
    foldPhones(link);
    return link;
  });
  keepBridgeAnswering();
  // Stops when nothing's paired any more, or when another bridge took over; up while the engine
  // is down too, so a phone can start it. Phones removed since lose what they have open, and the
  // box's keeper, if it went, starts again (it keeps this bridge, and the engine, running).
  let current: LinkHost | null = null;
  let connectedIdentity: { key?: string; secret?: string; relay?: string } | undefined;
  startTakeover(() => current);
  setInterval(() => {
    let owner: number | null = null;
    try { owner = Number(fs.readFileSync(PID_FILE, "utf8")); } catch {}
    const link = readLink();
    if (!needed(link)) {
      forgetPhones();
      noLongerNeeded();
    }
    if (!resident && !needed(link)) return void cleanUp("no longer needed");
    if (owner !== process.pid) return void cleanUp("ownership changed");
    current?.keepOnly(currentPhonePairings(link!));
    if (!resident) keepKeeper();
  }, 30_000);

  // Your phones' notifications, on whichever relay connection is up: the engine's news, and what
  // agents ask for as they browse.
  // False when it can't go now (the relay isn't there, or its connection is closing), to be tried again.
  const tellPhones = (notice: Notice): boolean => sendToPhones(current, readLink(), notice);
  // Each held a little while a client's in use here (PROTOCOL.md, "kuvo/attention"); what's held
  // is kept in run/held-notices.json, for the next bridge to send if this one stops first: read
  // once the one before has gone (a few seconds at most), so it neither sends nor holds one more.
  for (let tries = 0; tries < 60 && old && isBridge(old); tries++) await sleep(50);
  let held: KeptNotice[] = [];
  try { held = JSON.parse(fs.readFileSync(HELD_FILE, "utf8")); } catch {}
  const hold = new NoticeHold({
    send: tellPhones,
    held,
    save: entries => {
      try {
        if (entries.length) writePrivate(HELD_FILE, JSON.stringify(entries));
        else fs.rmSync(HELD_FILE, { force: true });
      } catch {}
    },
  });
  const dispatcher = createBoxDispatcher();
  let served: { port: number; password: string } | undefined;
  const local = dispatcher.entry({ hold, info: { name: os.hostname(), ...owner?.info() } });
  if (resident) {
    const password = crypto.randomBytes(24).toString("hex");
    const server = await listenBox(password, owner!.entry(local));
    const port = (server.address() as AddressInfo).port;
    socketKey();
    fenceOwnPort(port);
    served = { port, password };
  }
  const tell = (notice: Notice): boolean => hold.tell(notice);
  const answered = (id: string): void => hold.resolve(id);
  // Its engine and the coding agents here (their sessions, and group chats that live here),
  // through their bridge: each notice kept as told too, for clients to show (`kuvo/notices`).
  watchBox({ tell: notice => { keepTold(notice); return tell(notice); }, hold });
  if (resident) {
    sendWakes();
    stopWindowsAwake = keepWindowsAwake(async () => {
      const due = nextWake();
      return commandsWorking() || (due !== null && Math.abs(due - Date.now() / 1000) <= 120)
        || await agentsWorking() === true;
    });
  }
  // And what its agents send you on purpose (notify_user), whichever process answered them.
  watchSentByAgents(tell);
  // (So its clients know it tells them: a bridge from before didn't.)
  markTeller();
  // What its group chats send your other computers, carried on the relay connection that's up.
  watchSeats(() => current);
  // Moves to and from your other computers, carried on the same connection.
  watchMoves(() => current);
  watchAsks(tell, { answered });
  // Working outside a conversation's project asks first, as Settings › Permissions says
  // (engine/defaults.ts).
  keepEngineDefaults();
  // Agents' commands, in the sandbox wherever it can run (fence/state.ts), set up before the vault
  // goes by it.
  await keepFence();
  // Conversations' approval kept to what it comes to as the sandbox starts or stops.
  keepApprovals();
  // What its group chats go by, worked out here and given to the bridge that runs them.
  keepGroupSettings();
  // The branch prefix it kept before Settings › Git was each computer's, made its setting.
  keepOldBranchPrefix();
  // The vault, held here: its key in this process's memory, its asks told to your phones.
  // (With the coding agents' bridge: a conversation's asks are read here, theirs too.)
  const vault = holdVault(dispatcher.entry());
  if (resident && !vault.listening) await once(vault, "listening");
  setVaultTell(tell);
  watchVaultAsks(tell, answered);
  watchSandboxAsks(tell, answered);
  // What its bots ask you before they do it (making a project): told the same way.
  watchWorkAsks(tell, { answered });
  // Your other computers: this box's agents told to them while the relay connection is up (and
  // listed here, relay or not), and its agents' letters sent; without the relay a while, those
  // not sent yet go to the Mac.
  let lastWelcomed = Date.now();
  let lastSetup = "";
  let publishingDirectory = false;
  refreshOwn(readEngineFile).catch(() => {});
  // Your time zone in its bots' and conversations' folders, kept so as it runs.
  keepZone();
  keepFleetUpdates(liveLink);
  setInterval(() => { try { tellUntold(); } catch {} }, 60_000).unref?.();
  keepMachinesUp(automaticSshUpdate);
  // Recently Deleted's skills past their 30 days, gone with no client there too.
  keepSkillTrashClear();
  keepMarketplaceProjection();
  startMarketplaceRelay().catch(() => console.error("Marketplace connections could not start."));
  // What a Work Queue change left half read as the box stopped.
  clearStaging();
  setInterval(() => {
    if (current?.welcomed) announce(current, readEngineFile).catch(() => {});
    else refreshOwn(readEngineFile).catch(() => {});
    // Running: its Macs' 30 days go on, the relay there or not (time off doesn't count).
    try { noteOnline(); } catch {}
  }, 60_000);
  const sendNow = () => {
    try { sendOutbox(current ?? { send() {} }, undefined, Date.now(), current?.welcomed ? 0 : Date.now() - lastWelcomed); } catch {}
  };
  // What this box answers goes at once; what its tools leave (another process's), the moment the
  // system says it's there (a watch on the folder, the same on macOS and Linux; a moment's gather,
  // as a tool writes it in more than one step). One that dies is made again each round, which also
  // sends what a watch missed.
  outboxSoon.send = sendNow;
  let soon: ReturnType<typeof setTimeout> | null = null;
  const leftNow = () => {
    soon ??= setTimeout(() => { soon = null; sendNow(); }, 50);
  };
  try { fs.mkdirSync(SENDING, { recursive: true, mode: 0o700 }); } catch {}
  let outboxWatch = watchFolders([SENDING], leftNow, { recursive: false });
  // What you do to an item another computer keeps, left by a route (`box route` too): it goes at
  // once, and again each few seconds while it's waited for.
  const forwardNow = () => { try { sendForwards(current); } catch {} };
  forwardsSoon.send = forwardNow;
  try { fs.mkdirSync(FORWARDS, { recursive: true, mode: 0o700 }); } catch {}
  let forwardsWatch = watchFolders([FORWARDS], forwardNow, { recursive: false });
  // Edits to bots on your other computers held here (by a route, `box route` too): each goes to
  // its computer at once, again each minute while it's waited for, and as soon as it's back.
  const heldNow = () => { try { sendHeld(current); } catch {} };
  heldSoon.send = heldNow;
  // A computer that's away, reached again as a client asks (Connect, left by a route, `box route`
  // too): told this box's agents and asked for theirs, at once and at the next round.
  const reachNow = () => sendReachAgain(Boolean(current?.welcomed), key => void announce(current!, readEngineFile, readLink(), Date.now(), true, key).catch(() => {}));
  reachAgainSoon.send = reachNow;
  const identityChanged = (link: LinkState | null) => connectedIdentity &&
    (link?.key !== connectedIdentity.key || link?.secret !== connectedIdentity.secret || link?.relay !== connectedIdentity.relay);
  setInterval(() => {
    const identity = readLink();
    if (identityChanged(identity) || (resident && current && !relayNeeded(identity))) {
      current?.disconnect?.();
      return;
    }
    if (!outboxWatch.whole) {
      outboxWatch.close();
      outboxWatch = watchFolders([SENDING], leftNow, { recursive: false });
    }
    if (!forwardsWatch.whole) {
      forwardsWatch.close();
      forwardsWatch = watchFolders([FORWARDS], forwardNow, { recursive: false });
    }
    scheduleWipe();
    try { sendRemovalUpdates(current); } catch {}
    forwardNow();
    void refreshReceipts().catch(() => {});
    sendSetupCopies(current).catch(() => {});
    heldNow();
    reachNow();
    if (current) try { refreshPhoneTrust(current); } catch {}
    if (current?.welcomed && docsNoted()) {
      try {
        docsChanged(current, { tell: true });
        tellUntold();
        const link = readLink();
        if (link?.installerClaim && link.setup?.role === "member") {
          void announce(current, readEngineFile, link, Date.now(), true, link.setup.primary).catch(() => {});
        }
      } catch {}
      // (A device removed by another process, Kuvo Code beside this box: what it has open ends now.)
      try { current.keepOnly(currentPhonePairings(readLink())); } catch {}
    }
    if (current?.welcomed) lastWelcomed = Date.now();
    noteRelay(Boolean(current?.welcomed), lastWelcomed);
    sendNow();
    deliverLeftAnswers().catch(() => {});
    // The rows of sessions here whose lead lives on another computer, told to it as they change.
    if (current?.welcomed) {
      if (directoryChanged && !publishingDirectory) {
        directoryChanged = false;
        publishingDirectory = true;
        void announce(current, readEngineFile).catch(() => {}).finally(() => { publishingDirectory = false; });
      }
      void shareNotices(current).catch(() => {});
      void shareOutputs(current).catch(() => {});
      void shareArchived(current).catch(() => {});
      void shareLibrary(current).catch(() => {});
      void shareSetupLists(current).catch(() => {});
      pushRows(current, readLink(), peerTargets()).catch(() => {});
      try { pushActivity(current, readLink(), localActivity()); } catch {}
    }
    // The Work Queue's ask for more numbers, to the primary, while one is under way; and this box's
    // queue told to your other computers as it changes, with what it's handing to theirs.
    // Your setup, as this box leads it or follows it (once a minute), before its numbers go by it.
    noteSetupNow();
    const setup = readLink()?.setup;
    const position = { epoch: setup?.epoch, role: setup?.role, primary: setup?.primary };
    const stamp = JSON.stringify(position);
    if (stamp !== lastSetup) {
      lastSetup = stamp;
      report("setup position", position);
    }
    void pushNumbers(current).catch(() => {});
    if (current?.welcomed) pushQueues(current, readLink(), peerTargets()).catch(() => {});
    else try { handUpkeepSoon(readLink()); } catch {}
    // (Once: what was being worked on when the Work Queue's items moved over to statuses.)
    void settleStarted(() => atWorkForSure(AbortSignal.timeout(10_000)));
  }, 3_000);

  if (served) {
    await serveSshPipe(owner!.entry(local));
    if (clientServerStopped()) return cleanUp("turned off or paused");
    scheduleWipe();
    writeServeRecord({ ...served, pid: process.pid, kept: false, startedAt, generation: owner!.generation });
    console.log(JSON.stringify({ url: `http://127.0.0.1:${served.port}` }));
  }
  let attempt = 0;
  while (true) {
    const link = readLink();
    if (resident && (!relayNeeded(link) || !link?.key || !link.secret || !link.relay)) {
      attempt = 0;
      await sleep(3_000);
      continue;
    }
    if (!resident && !needed(link)) {
      forgetPhones();
      noLongerNeeded();
      return cleanUp("no longer needed");
    }
    let engine: Partial<EngineRecord> = {};
    try { engine = readEngineFile(); } catch {}
    const route: LinkHandler = (request, actor, signal) => handler(request, actor, signal);
    const host: LinkHost = new LinkHost({
      key: Buffer.from(link!.key!, "base64url"),
      hello: { v: 1, kind: "machine", name: link!.name || os.hostname(), app: BRIDGE_VERSION, engine: engine.version ?? null },
      authorize: (key, hello, address, signal) => deviceConnectionsAllowed(readLink()) && authorizeDeviceConnection(host, key, hello, address, signal),
      confirmed: confirmPhone,
      mail: (from, body) => receiveMail(body, host, (sender, payload) => peerMail(sender, payload, host, peerPlaces), (mac, given, at) => hold.attend(`mac:${mac}`, given, at), from),
      handle: owner ? owner.entry(route) : route,
      connect: engineConnect(readEngineFile, (): number => host.unsent?.() ?? 0),
      send: () => {},
    });
    const handler = dispatcher.entry({
      info: { name: link!.name || os.hostname() },
      phonesChanged: changed => host.keepOnly(currentPhonePairings(changed)),
      hold,
    });
    current = host;
    machineLink.host = host;
    connectedIdentity = link!;
    const started = Date.now();
    const connection = connectOnce(link!.relay!, Buffer.from(link!.secret!, "base64url"), host, connected => {
      report("relay welcomed");
      machineWelcomed(connected);
    }, (reason, welcomed) => report("relay ended", { reason, welcomed, elapsedMs: Date.now() - started }));
    const welcomed = await (resident ? connection.catch(error => {
      report("relay ended", { reason: String(error), welcomed: false });
      return false;
    }) : connection);
    if (resident) {
      const changed = identityChanged(readLink());
      current = null;
      machineLink.host = null;
      connectedIdentity = undefined;
      if (changed) {
        attempt = 0;
        continue;
      }
    }
    const next = nextTry(attempt, welcomed, Date.now() - started);
    attempt = next.attempt;
    await sleep(next.wait);
  }
}

async function restoreLabelsCommand(args: string[]): Promise<number> {
  let file: string;
  let dryRun: boolean;
  let force: boolean;
  try {
    const { positionals, values } = parseArgs({
      args, allowPositionals: true, options: { "dry-run": { type: "boolean" }, force: { type: "boolean" } },
    });
    if (positionals.length !== 2 || positionals[0] !== "restore") throw new Error("Expected a restore file.");
    file = positionals[1]!;
    dryRun = values["dry-run"] === true;
    force = values.force === true;
  } catch {
    console.error("Usage: kuvo-link.js box labels restore <file> [--dry-run] [--force]");
    return 2;
  }
  let library: unknown;
  try {
    const maxBytes = 64 * 1024;
    const buffer = Buffer.alloc(maxBytes + 1);
    const source = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK);
    try {
      if (!fs.fstatSync(source).isFile()) throw new Error("Use a saved JSON file.");
      const length = fs.readSync(source, buffer, 0, buffer.length, 0);
      if (length > maxBytes) throw new Error("The saved list exceeds 64 KiB.");
      library = JSON.parse(buffer.subarray(0, length).toString("utf8"));
    } finally { fs.closeSync(source); }
  } catch (error) {
    console.error(`Couldn't read ${file} as JSON: ${(error as Error).message}`);
    return 1;
  }
  const answer = await askHolder({
    method: dryRun ? "GET" : "POST", path: LABEL_RESTORE_ROUTE, headers: {},
    body: Buffer.from(JSON.stringify({ library: isObject(library) ? library.labels : library, force })),
  }, AbortSignal.timeout(30_000));
  if (!answer) {
    console.error("The local Kuvo box didn't answer. Check that it's running and try again.");
    return 1;
  }
  const value = parsed(Buffer.from(answer.body, "base64"));
  if (answer.status !== 200 || !fitsShape(LABEL_RESTORE_RESULT, value)) {
    console.error(isObject(value) && typeof value.message === "string" ? value.message : `Label restore failed (${answer.status}).`);
    return 1;
  }
  const result = value as Infer<typeof LABEL_RESTORE_RESULT>;
  if (dryRun && result.provisional) console.log("The shared label library is syncing. This preview uses the main computer’s current labels.");
  console.log(`${dryRun ? "Dry run: would restore" : "Restored"} ${result.restored}, already present ${result.alreadyPresent}, conflicts ${result.conflicts}.`);
  for (const detail of result.details) console.log(`${detail.id}: ${detail.reason}`);
  return 0;
}

// `kuvo-link.js box <command>`: a Mac's way to a machine's own settings, over SSH, the same way a
// phone's goes (one answer, JSON, on stdout). `adopt` (a seed on stdin), `read`, `change`
// ({changes} on stdin), `key <name>` (its value on stdin; none removes it), `route` (a request,
// {method, path, body?}, answered as a phone's is: {status, body}).
async function box(command: string, [flag, internalPath]: string[], useSshPipe: boolean): Promise<void> {
  const input = async (): Promise<string> => {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    return Buffer.concat(chunks).toString("utf8");
  };
  const say = (value: unknown): void => console.log(JSON.stringify(value));
  switch (command) {
    case "wsl-startup":
      try { if (startWslTask(flag)) return; } catch (error) { console.error(String(error)); }
      console.error("Kuvo Code couldn’t set up Windows sign-in startup. Sign in to Windows, check that WSL interop is enabled, then try again.");
      process.exitCode = 1;
      return;
    case "tool-command-worker":
      return (await import("./tool-command")).toolCommandWorker(flag, input);
    case "backups": {
      // The owner already has OS access to this Box's private files. This is a
      // separate command, with no way to forward its authority through box route.
      const action = process.argv[4] ?? "status";
      const get = action === "status" || action === "report";
      if (process.argv.length > 5 || (!get && !["destination", "now", "points", "erase", "restore", "match"].includes(action))) {
        console.error("Usage: kuvo-link.js box backups [status|report|destination|now|points|erase|restore|match] (JSON on stdin for changes)");
        process.exitCode = 2;
        return;
      }
      const handler = createBoxDispatcher().entry();
      const route = "kuvo/box/backups" + (action === "report" ? "" : "/" + action);
      const body = get || action === "now" ? undefined : Buffer.from(await input());
      let status = 500;
      const chunks: Buffer[] = [];
      for await (const piece of handler({ method: get ? "GET" : "POST", path: route, headers: {},
        served: true, fromApp: true, body }, {})) {
        if ("head" in piece) status = piece.head.status;
        else chunks.push(piece.body);
      }
      const value = parsed(Buffer.concat(chunks));
      if (isObject(value)) {
        // Configure keeps the key in the owner's mode-0600 configuration.
        const { recoveryKey: _key, ...answer } = value;
        say({ ...answer, status });
      } else say({ status });
      process.exitCode = status >= 400 ? 1 : 0;
      return;
    }
    case "labels":
      process.exitCode = await restoreLabelsCommand(process.argv.slice(4));
      return;
    // COMPAT(0.7 settings): `box adopt`, which a Mac runs over SSH. Goes 3 months after 0.8.0.
    case "adopt": {
      let seed: { settings?: unknown; keys?: unknown } | null = null;
      let adopted: ReturnType<typeof adoptBox>;
      try {
        const value: unknown = JSON.parse((await input()) || "null");
        if (value !== null) seed = settingsSeedFormat.read(value);
        upgradeLinkFolder();
        adopted = adoptBox(seed);
      } catch (error) {
        // Said as it is, for the Mac to show.
        if (!(error instanceof Error)) throw error;
        console.error(error.message);
        process.exitCode = 1;
        return;
      }
      // Its settings its own now: the branch prefix it kept before, taken into them.
      keepOldBranchPrefix();
      // As its catalogs have them now: a Mac runs this as it connects, having just put newer ones here,
      // whether or not this box has a bridge or starts its engine again. (Stays when adopting goes.)
      await upgradeIntegrations().catch(() => {});
      return say({ ...adopted, settings: readSettings().settings ?? adopted.settings });
    }
    // COMPAT(beta.1 box-read-change): `box read`, `box change` and `box key`, which beta.1 runs over SSH for everything its Settings change on a machine (`box route` took their place). Goes on the user's word, once no Mac from before 1.0 connects.
    case "read": {
      const { settings, problem } = readSettings();
      return say(problem ? { status: 409, message: problem } : { status: 200, settings, own: ownsSettings() });
    }
    case "change": {
      let given: { changes?: unknown } | null = {};
      try { given = JSON.parse(await input()); } catch {}
      // (Checked there.)
      return say(ownsSettings() ? changeSettings(given?.changes) : { status: 409, message: "This machine's settings aren't its own yet." });
    }
    // The accounts a computer starts with, from the box that adds it (`add/seed.ts`): on its standard
    // input, never in its arguments.
    case "seed":
      return say(await takeSeed(await input(), readEngineFile, upgradeLinkFolder));
    case "key": {
      const name = process.argv[4];
      const value = await input();
      return say(setBoxKey(name, value.length ? value : null));
    }
    case "resident-info": {
      const info = await liveResidentInfo();
      if (info) say(info);
      else process.exitCode = 1;
      return;
    }
    case "update-status":
      return say({ status: 200, ...updateStatus() });
    case "update-now": {
      const started = requestUpdateNow();
      return say(started.status === 409 ? started : started.message ? { status: 202, ...updateStatus(), message: started.message } : { status: 202, ...updateStatus(), updating: "checking" });
    }
    case "route":
    case "route-to": {
      const addressed = command === "route-to";
      const internal = !addressed && flag === "--internal";
      const ssh: SshCommand = { addressed, internal, ...(internal && internalPath ? { internalPath } : {}) };
      if (useSshPipe) return pipeSshRoute(ssh);
      const stop = new AbortController();
      const abort = () => { stop.abort(); process.stdin.destroy(); };
      process.once("SIGTERM", abort);
      process.stdout.once("error", abort);
      try {
        const handle = createBoxDispatcher().entry({ front: addressed && IS_MAC ? macCompatibilityFront : undefined });
        await answerSshRoute(ssh, process.stdin, bytes => writeSshBytes(process.stdout, bytes), stop.signal, handle, true);
      } finally {
        process.stdin.destroy();
        process.off("SIGTERM", abort);
        process.stdout.off("error", abort);
      }
      return;
    }
    case "github-read":
      // A read of GitHub that was due as `route` answered, made here, outliving it.
      return void await readApart(process.argv[4], await input()).catch(() => {});
    case "reconnect": {
      // An integration's server connected again everywhere, a moment after `route` changed its
      // keys, once the engine has read them.
      const id = process.argv[4];
      await new Promise(done => setTimeout(done, 2_000));
      let engine;
      try { engine = readEngineFile(); } catch { return; }
      if (id) await reconnectEverywhere(engine, id);
      return;
    }
    case "personalities": {
      // A Mac's copy met with this one's: both end up as one, and the bridge tells your other
      // computers if this one changed.
      let given: GivenLibrary | null = null;
      try { given = JSON.parse((await input()) || "null"); } catch {}
      let legacy: { library: ReturnType<typeof readPersonalities>; changed: boolean } | undefined;
      updateLink(link => {
        if (usesAcceptedSaves(link?.setup)) return null;
        const before = JSON.stringify(readPersonalities());
        const library = changePersonalities(current => mergePersonalities(current, given));
        legacy = { library, changed: JSON.stringify(library) !== before };
        return null;
      });
      if (legacy) {
        if (legacy.changed) noteDocsChanged();
        return say({ status: 200, ...legacy.library });
      }
      const saved = await savePersonalityLibrary(given);
      if (saved.status === 200) {
        const reply: SaveEnvelope<CommittedReceipt> & typeof saved.library & { status: 200; operation: string } = {
          ...saved.library, operation: saved.receipt.operation, save: saved.receipt, status: 200,
        };
        return say(reply);
      }
      return say({ ...saved.refusal, status: saved.status });
    }
    default:
      console.error("Usage: kuvo-link.js box adopt | read | change | key <name> | route | backups | reconnect <server> | personalities | labels restore <file> [--dry-run] [--force] | update-status | update-now");
      process.exit(2);
  }
}

/** The first line on stdin, as JSON (what the app sends `serve` there, before anything else). */
function firstLine(): Promise<Record<string, unknown>> {
  return new Promise(resolve => {
    const given: Buffer[] = [];
    const take = (chunk: Buffer | string) => {
      const bytes = typeof chunk === "string" ? Buffer.from(chunk) : chunk;
      const end = bytes.indexOf(10);
      given.push(end < 0 ? bytes : bytes.subarray(0, end));
      if (end < 0) return;
      process.stdin.off("data", take);
      process.stdin.off("end", done);
      process.stdin.pause();
      try { resolve(JSON.parse(Buffer.concat(given).toString("utf8"))); } catch { resolve({}); }
    };
    const done = () => resolve({});
    if (process.stdin.readableEnded) return done();
    process.stdin.on("data", take);
    process.stdin.once("end", done);
    process.stdin.resume();
  });
}

/** The coding agents' bridge beside this box's engine, ended should it hang (it's started again as after
 * a crash), wherever this runs. The link services watch the engine independently. */
function keepBridgeAnswering(): void {
  new HangWatch({ name: "bridge", run: RUN, record: BRIDGE_FILE, isIt: runsBridge }).start();
}

function removeOwnLinkRecord(): void {
  try { if (fs.readFileSync(PID_FILE, "utf8") === String(process.pid)) fs.unlinkSync(PID_FILE); } catch {}
}

function removeOwnServeRecord(generation?: string): void {
  try {
    const record = readServeRecord();
    if (record.pid === process.pid && record.generation === generation) fs.rmSync(SERVE_FILE(), { force: true });
  } catch {}
}

async function listenBox(password: string, handle?: LinkHandler) {
  const server = serveBox(password, handle);
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return server;
}

/** `serve --kept` (serve-kept.ts): the box's server for clients, which its keeper keeps running
 * while one wants it (`run/serve.needed`), answering until it isn't wanted any more (looked at
 * every 30 s) or another took its place. With a password of its own, said in `run/serve.json`. */
async function keptServe(): Promise<void> {
  if (residentRunning()) return;
  // One at a time: another that runs stays.
  const other = runningServe({ whenUnknown: true });
  if (other && other.pid !== process.pid) {
    console.error(`The box's server already runs (${other.pid}).`);
    return;
  }
  if (!fs.existsSync(SERVE_NEEDED())) {
    console.error("No client wants the box's server here (run/serve.needed).");
    return;
  }
  const password = crypto.randomBytes(24).toString("hex");
  const server = await listenBox(password);
  const port = (server.address() as AddressInfo).port;
  const published = locked(machineServerStartLock, () => {
    // A resident or another kept listener can have claimed the box while listen was pending.
    const serving = runningServe({ whenUnknown: true });
    if (residentRunning() || (serving && serving.pid !== process.pid) || !fs.existsSync(SERVE_NEEDED())) return false;
    initializeMachineSetup();
    try { socketKey(); } catch {}
    writeServeRecord({ pid: process.pid, port, password, kept: true });
    return true;
  }, { exclusive: true });
  if (!published) return void server.close();
  const mine = () => {
    try { return readServeRecord().pid === process.pid; } catch { return false; }
  };
  const leave = () => {
    removeOwnServeRecord();
    process.exit(0);
  };
  process.on("SIGTERM", leave);
  process.on("SIGINT", leave);
  (await import("./backup/engine")).keepDailyBackups();
  // New code put in place (an update): it goes, for the keeper to start it again on it.
  leaveForNewCode(fileURLToPath(import.meta.url), leave);
  // With nobody paired or enrolled, only the local server is running.
  startTakeover(() => null, () => !needed(readLink()));
  console.log(JSON.stringify({ url: `http://127.0.0.1:${port}` }));
  // Not wanted any more, or its record no longer names it. (A
  // test's box looks as often as it says.)
  const every = !REAL_BOX && Number(process.env.KUVO_TEST_SERVE_EVERY) > 0 ? Number(process.env.KUVO_TEST_SERVE_EVERY) : 30_000;
  scheduleWipe();
  setInterval(() => {
    if (!fs.existsSync(SERVE_NEEDED()) || !mine()) leave();
    scheduleWipe();
  }, every);
}

// The app's own box has no shell keeper. Its HTTP server keeps the services subprocess by stdin.
function keepAppServices(): void {
  const child = spawn(process.execPath, ["--config=/dev/null", "--no-env-file", fileURLToPath(import.meta.url), "box-services", "--app"], {
    env: { ...process.env, BUN_BE_BUN: "1" }, stdio: ["pipe", "ignore", "inherit"],
  });
  child.stdin?.on("error", () => {});
  child.on("error", error => console.error("Box services:", error.message));
  child.once("close", () => setTimeout(keepAppServices, 5_000).unref());
}

function keepResidentServices(): () => void {
  let stopped = false;
  let leave: (() => void) | undefined;
  let retry: NodeJS.Timeout | undefined;
  const start = async () => {
    try { leave = await openBoxServices(false, () => !stopped, true); }
    catch (error) { console.error("Box services:", error); }
    if (!stopped && !leave) retry = setTimeout(start, 2_000).unref();
  };
  void start();
  return () => {
    stopped = true;
    clearTimeout(retry);
    leave?.();
  };
}

function bridgeOwnsServices(app: boolean): boolean {
  try {
    const record = readBridgeFile();
    if (record.boxServices === true || !processRuns(record.pid, "kuvo-agents")) return false;
    if (app) {
      try { if (fs.readFileSync(path.join(RUN, "bridge-box-services.pid"), "utf8").trim() === String(record.pid)) return false; } catch {}
    }
    return true;
  } catch { return false; }
}

async function openBoxServices(app: boolean, wanted = () => true, resident = false): Promise<(() => void) | undefined> {
  if (typeof Bun === "undefined") return;
  const script = fileURLToPath(import.meta.url);
  const { serveBoxServices } = await import("./services");
  const home = path.join(APP_FOLDER ?? K, "agents");
  if (bridgeOwnsServices(app)) return;
  if (!resident) process.env.KUVO_AGENTS_HOME = home;
  if (!app) {
    const login = OWN_HOME_MAC && REAL_BOX ? await loginEnvironment() : undefined;
    for (const name of ["CODEX_HOME", "GROK_HOME", "CLAUDE_CONFIG_DIR", "CURSOR_API_KEY", "GH_CONFIG_DIR", "GH_TOKEN", "GITHUB_TOKEN"]) {
      if (login?.[name] !== undefined) process.env[name] = login[name];
    }
    const loginPath = login?.PATH ?? savedLoginPath(K);
    if (loginPath) process.env.PATH = joinPath(path.join(K, "bin"), loginPath);
  }
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  const leave = locked(path.join(RUN, ".box-services-start"), () => {
    if (!wanted() || fs.existsSync(path.join(RUN, ".rolling-back")) || (!resident && (updateUnderWay(RUN) || residentRunning()))) return;
    if (!app && fs.existsSync(path.join(RUN, "agents", "handover.json"))) {
      try {
        const previous = fs.readFileSync(path.join(RUN, "agents", "previous", "kuvo-agents.js"), "utf8");
        if (!/^\/\/ Box services ownership v1$/m.test(previous)) return;
      } catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") return; }
    }
    const current = readServices(RUN);
    if ((current && (processRuns(current.pid, "box-services") || residentRunning(current.pid))) || bridgeOwnsServices(app)) return;
    return serveBoxServices({
      home, run: RUN, box: APP_FOLDER ?? K, bots: path.join(APP_FOLDER ?? K, "bots"), artifacts: path.join(K, "artifacts"),
      engineData: APP_FOLDER ? path.join(APP_FOLDER, "opencode", "data", "opencode") : path.join(K, "data", "opencode"),
      bundle: script,
    });
  }, { exclusive: true });
  return leave;
}

// Compatibility for releases and old app copies that launch services separately.
async function runBoxServices(): Promise<void> {
  const app = process.argv.includes("--app");
  if (app !== Boolean(APP_FOLDER)) return;
  const close = await openBoxServices(app);
  if (!close) return;
  const leave = () => { close(); process.exit(0); };
  process.on("SIGTERM", leave);
  process.on("SIGINT", leave);
  if (app) {
    process.stdin.on("end", leave);
    process.stdin.resume();
  } else leaveForNewCode(fileURLToPath(import.meta.url), leave);
  setInterval(() => {
    if (residentRunning() || bridgeOwnsServices(app) || ["off", "paused"].some(name => fs.existsSync(path.join(RUN, name)))) leave();
  }, 2_000).unref();
}

const serveInputFormat = additiveObjectFormat("Box serve input");

async function serve(owner?: ClientOwner): Promise<void> {
  if (process.argv.includes("--resident")) {
    if (process.argv.includes("--kept") || APP_FOLDER) throw new Error("Use serve --resident on an installed box.");
    if (!IS_MAC) {
      return ownClientServer(async owner => {
        upgradeLinkFolder();
        try { await run(owner); }
        catch (error) {
          removeOwnServeRecord(owner.generation);
          removeOwnLinkRecord();
          throw error;
        }
      });
    }
  }
  if (process.argv.includes("--kept")) {
    if (residentInstalled()) return keepKeeper();
    upgradeLinkFolder();
    return keptServe();
  }
  if (OWN_HOME_MAC && !owner) return ownClientServer(async owner => {
    try { await serve(owner); }
    catch (error) {
      removeOwnServeRecord(owner.generation);
      removeOwnLinkRecord();
      throw error;
    }
  });
  const startedAt = Date.now();
  // Its password, and this Mac's vault's secrets, from the app on stdin (never its environment,
  // which any of your processes can read); a test's box, in OPENCODE_PASSWORD.
  let password = process.env.OPENCODE_PASSWORD;
  delete process.env.OPENCODE_PASSWORD;
  // Whether the helper that started it leaves by itself for a new build of it (`renews`, said by one
  // since beta.31); undefined where it wasn't started so.
  let given: Record<string, unknown> | undefined;
  if (!password) {
    given = serveInputFormat.read(await firstLine());
    if (typeof given.password === "string") password = given.password;
  }
  if (!password) {
    console.error("serve answers only with a password: on its stdin, {\"password\"}, or in OPENCODE_PASSWORD.");
    process.exit(2);
  }
  if (APP_FOLDER) readServed(RUN);
  if (owner) {
    selectSshPipe();
    writePrivate(PID_FILE, String(process.pid));
  }
  upgradeLinkFolder();
  (await import("./backup/engine")).keepDailyBackups();
  if (given) takeVaultSecrets(given);
  const renews = given && given.renews === true;
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  // (Sessions' turns' lock folders left by a box cut off: `engine/notes.ts`.)
  clearNotesLocks();
  // This Mac's settings with what its engine starts with, before a client reads them.
  if (IS_MAC) addMacDefaults();
  else initializeMachineSetup();
  // And as its catalogs have them now, before it says where it is: this Mac's engine starts only after
  // that. (Any key to take away comes after, below: its app may not answer yet, and an engine
  // running takes settings as they change.)
  try { upgradeBeforeEngine(); } catch {}
  // What's left of its bots' own keys gone (bots/own-keys.ts), before it says where it is too: all
  // of it where its keys are its own files; where its app keeps them, its bots' folders' settings
  // now, and the keys once its app answers (below).
  if (KEYS_ON_HOST) { try { dropBotsOwnServers(runningEngine); } catch {} } else await dropBotsOwnKeys(runningEngine).catch(() => {});
  // This Mac's notices for your phones, held while you're at a computer, as a machine's are.
  if (IS_MAC) macLink.hold = macNoticeHold();
  const dispatcher = createBoxDispatcher();
  const local = dispatcher.entry({ hold: macLink.hold, info: { name: os.hostname(), ...owner?.info() } });
  const server = await listenBox(password, owner ? owner.entry(local) : local);
  const port = (server.address() as AddressInfo).port;
  // The key to its sockets, and agents' commands in the sandbox (fence/state.ts), which refuses them
  // this server too, there before it says where it is (holdVault, below, makes it too).
  try { socketKey(); } catch {}
  fenceOwnPort(port);
  await keepFence();
  keepApprovals();
  keepGroupSettings();
  keepEngineDefaults();
  // The vault, held here (on a machine, `run` holds it; a test's box serves alone), its asks told
  // to your phones through this Mac's link: listening before it says where it is, so what reaches
  // the vault from then on finds it open.
  holdVault(dispatcher.entry());
  if (owner) {
    await serveSshPipe(owner.entry(local), owner.entry(dispatcher.entry({ hold: macLink.hold, front: macCompatibilityFront })));
    if (clientServerStopped()) {
      removeOwnLinkRecord();
      process.exit(0);
    }
  }
  if (APP_FOLDER) {
    writeSecret(path.join(RUN, "tool-http.json"), JSON.stringify({ port, password }));
    if (typeof Bun !== "undefined") keepAppServices();
  }
  // A Mac's box in a home of its own says where it answers itself, for its clients (`run/serve.json`,
  // as one kept for them does), before it says so on stdout: its helper started it, and no app
  // writes it (PROTOCOL.md, "This Mac's box: its helper"). Taken away as it leaves.
  if (owner) keepEngineOnDemand();
  const stopServices = owner ? keepResidentServices() : undefined;
  if (owner) writeServeRecord({ pid: process.pid, port, password, kept: false, startedAt, generation: owner.generation });
  console.log(JSON.stringify({ url: `http://127.0.0.1:${port}` }));
  // This Mac's vault's newest generations, from its app, before agents use it (vault/store.ts).
  if (IS_MAC) void (async () => {
    for (let tries = 0; tries < 120; tries++) {
      try { return await primeGenerations(); } catch {}
      await sleep(1000);
    }
  })();
  if (IS_MAC) {
    setVaultTell(macTell);
    watchVaultAsks(macTell, id => macLink.hold?.resolve(id));
    watchSandboxAsks(macTell, id => macLink.hold?.resolve(id));
  }
  // Any integration's key to take away for its maker's server (the rest is done, above), and its
  // bots' own keys where its app keeps them: where they can't be reached yet, it's tried again.
  upgradeIntegrations().catch(() => {});
  if (KEYS_ON_HOST) keepBotsOwnKeysGone(runningEngine);
  // A Mac's box in a home of its own: the files its engine reads its keys from, kept as its host
  // keeps them (its app did that in its folder), and its Mac kept awake while agents work, as its
  // setting says (its app's keeper did, by the app's).
  keepKeyFiles();
  // What a move into it that's done still owes that asks nothing, settled beside it.
  try { settleOwedAtStart(); } catch (error) { console.error(`What the move still owes couldn't be settled now: ${(error as Error).message}`); }
  keepAwakeWhileWorking(() => agentsWorking());
  // And their conversations and places in group chats, where those weren't answering.
  finishDeletes(readEngineFile, bridgeTarget).catch(() => {});
  keepBridgeAnswering();
  // Your time zone in its bots' and conversations' folders, kept so as it runs.
  keepZone();
  // Its wake-ups and watches, sent while it runs, with the app's window closed too.
  sendWakes();
  // This Mac's link to your phones and machines, as its app last said it's wanted (kept in
  // run/mac-link.json, so it's back on the relay after a restart before the app opens), or once it says so.
  if (IS_MAC) runMacLink(owner ? { entry: options => owner.entry(dispatcher.entry(options)) } : dispatcher).catch(() => {});
  // The machines it set up over SSH, brought up to date with it once nothing is at work there.
  if (IS_MAC) keepMachinesUp();
  // Its agents and their letters, and its group chats' mail, for your other computers. (Not for
  // the app's route scenarios, `KUVO_TEST_NO_PEERS`, whose fixture tells its bridge who they are.)
  // Never a real box (a test's folder only).
  const testsAlone = process.env.KUVO_TEST_NO_PEERS && !REAL_BOX;
  if (IS_MAC && !testsAlone) {
    runMacPeers();
    keepFleetUpdates(liveLink);
    startTakeover(liveLink);
  }
  // Recently Deleted's skills past their 30 days, gone with no client there too.
  keepSkillTrashClear();
  if (IS_MAC) {
    keepMarketplaceProjection();
    startMarketplaceRelay().catch(() => console.error("Marketplace connections could not start."));
  }
  // What a Work Queue change left half read as the box stopped.
  clearStaging();
  // Its bots' jobs kept in their folders, taken once from where they were before, once its app
  // makes them this box's (it gives the box its kit then).
  if (readKit()) try { keepJobs(); } catch {}
  // The app's box, started (its upgrades run as it starts, its bots' jobs taken): said, with the
  // app's version and its own, for a move of it into a home of its own, which waits for that
  // (`rehome`). A box in a home of its own lets its kept copy go once its 30 days are up.
  try {
    if (APP_FOLDER) {
      readServed(RUN);
      writePrivate(path.join(RUN, SERVED_FILE), JSON.stringify(servedFormat.write({ app: MAC_APP_VERSION, bridge: BRIDGE_VERSION, at: Date.now() })));
    }
    else clearKept(K);
  } catch {}
  // Its bots kept told what they are as their runs start, on the engine and with the coding agents;
  // on a Mac, its news for your phones too (while the phone link's on), and its sign-ins that ended.
  // Each notice kept as told too, for its clients to show (`kuvo/notices`); and what its agents send
  // you on purpose (notify_user).
  watchBox({ tell: notice => { keepTold(notice); return macTell(notice); }, hold: macLink.hold, bots: true, notices: IS_MAC });
  if (IS_MAC) watchSentByAgents(macTell);
  // Its app gone (its stdin closed): so is it, once what its link has queued for the relay is out
  // (a notice sent as the app quit), a moment at most.
  let leaving = false;
  const leave = async (): Promise<void> => {
    if (leaving) return;
    leaving = true;
    await owner?.drain();
    stopServices?.();
    for (let tries = 0; tries < 30 && (macLink.host?.unsent?.() ?? 0) > 0; tries++) await sleep(50);
    // What didn't get out in time isn't lost without a word.
    if ((macLink.host?.unsent?.() ?? 0) > 0) console.error("Kuvo Code quit before what its link had for your phones and machines went out.");
    if (owner) {
      removeOwnServeRecord(owner.generation);
      removeOwnLinkRecord();
    }
    process.exit(0);
  };
  // The box's files changed under it (an update put new ones there): it goes, as its app gone does,
  // for its helper to start the new ones (only where its helper starts it, from the box's own files).
  leaveForNewCode(fileURLToPath(import.meta.url), () => void leave());
  // COMPAT(beta.30 helper-handover): a helper from before beta.31, which never leaves for the new build in its place, handed over once. Goes once no box is from before beta.31.
  watchHelperBuild(renews);
  // The copy of the engine's data an update kept, gone after its 30 days (a Mac runs no link whose
  // rounds would): as it starts, then every half hour.
  const dropBackups = () => { try { dropOldBackups(); } catch {} };
  dropBackups();
  setInterval(dropBackups, 1_800_000).unref?.();
  if (owner) {
    process.on("SIGTERM", leave);
    process.on("SIGINT", leave);
  }
  process.stdin.on("end", leave);
  process.stdin.on("close", leave);
  process.stdin.resume();
}

/** What a Mac's `setup` sends on stdin: the relay, this machine's name, the Mac's key and signing
 * key, the documents it gives, its id for this machine, and (its ID) whether it leaves a newer box
 * as it is; from a beta.1 Mac, its peers as it lists them. Each checked as it's read. */
interface SetupInput {
  relay?: string;
  name?: string;
  mac?: string;
  macSign?: string;
  /** The Mac's own name. */
  macName?: string;
  docs?: SignedDoc[];
  id?: string;
  macID?: string;
  tolerates?: number;
  peers?: unknown[];
  peersAt?: number;
  at?: number;
  moved?: unknown[];
  /** The Mac's keys before (made anew since), which it no longer is: let go here. */
  replaces?: unknown;
  /** Only these live key pairs may be carried through this SSH enrollment. */
  keepMemberships?: unknown;
  /** Revoked static keys whose wipe still needs to run. */
  blockedMemberships?: unknown;
  discardLocalPairings?: boolean;
}

// What the Mac sends: the relay, this machine's name, and the phones you've paired with the Mac,
// which may reach it directly. Keeps the machine's own keys (made the first time, and never
// leaving it) and phones paired with the machine itself, starts the bridge or restarts it (on
// another relay, or a newer one), and prints the machine's address and public key for the Mac
// to tell your phones. Nothing left to answer: the bridge stops, and the keys stay.
function readSetupReservation(): string | undefined {
  const file = path.join(path.dirname(K), ADD_RESERVATION);
  const marker = fs.lstatSync(file, { throwIfNoEntry: false });
  if (!marker) return undefined;
  if (!marker.isSymbolicLink()) return "";
  try { return fs.readlinkSync(file); } catch { return ""; }
}

async function setup(): Promise<void> {
  const token = process.argv[3], reserved = readSetupReservation();
  if ([token, reserved].some(value => value !== undefined && !isSetupReservation(value))) {
    console.error(UNSUPPORTED_RESERVATION);
    process.exit(2);
  }
  let input: SetupInput;
  try {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    input = setupInputFormat.read(JSON.parse(Buffer.concat(chunks).toString("utf8")));
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) {
      console.error(error.message);
      process.exit(2);
    }
    console.error("setup takes JSON on stdin: {relay, name, mac, macSign, docs: [{d, s}], id, …}");
    process.exit(2);
  }
  if (reserved !== token) {
    console.error("This OS account already has a Kuvo setup. Connect to your own OS account.");
    process.exit(SETUP_OCCUPIED);
  }
  upgradeLinkFolder();
  if (setupLayoutDeferred(K)) {
    console.error("Wait for the Kuvo Code update to finish before joining a setup.");
    process.exit(1);
  }
  const mac = typeof input.mac === "string" ? input.mac : undefined;
  // A legacy SSH add supplies only signed documents. Honor its own revocation
  // even when the client does not send the live roster separately.
  let offeredRoster: ReturnType<typeof readDoc> = null;
  let offeredPhones: ReturnType<typeof readDoc> = null;
  if (mac && input.macSign && Array.isArray(input.docs)) for (const envelope of input.docs) {
    const doc = readDoc(envelope);
    if (doc?.kind === "phones" && doc.origin === input.macSign && (!offeredPhones || doc.n > offeredPhones.n)) offeredPhones = doc;
    if (doc?.kind === "peers" && doc.origin === input.macSign && (!offeredRoster || doc.n > offeredRoster.n)) offeredRoster = doc;
  }
  const revocations = new Set(Array.isArray(offeredRoster?.body.left) ? offeredRoster.body.left : []);
  const offeredSetup = ledSetup(offeredRoster);
  const running = runningBridge();
  const old = readLink();
  let link = null as LinkState | null;
  let updatedLibrary: ReturnType<typeof readPersonalities> | undefined;
  // The Mac's keys before, made anew since (over SSH, so this box knows they were that Mac's).
  const replaced = mac && Array.isArray(input.replaces) ? input.replaces.filter((key): key is string => typeof key === "string" && KEY.test(key) && key !== mac).slice(0, 8) : [];
  let formerMacs: string[] = [];
  let alreadySetUp = false;
  let blockedMembership = false;
  let blockedPairings = false;
  updateLink(current => {
    if (readSetupReservation() !== token) {
      alreadySetUp = true;
      return null;
    }
    // Recheck ownership under the same lock used to enroll a peer. A primary that gained
    // members after the SSH probe cannot be enrolled into a different setup.
    if (current?.localWipe || current?.localUnlink || current?.removalSource || current?.setupMerge || activeInstallerClaim(current)) {
      alreadySetUp = true;
      return null;
    }
    if (mac && current?.setup?.role === "member") {
      const setup = current.setup;
      // SSH can refresh an existing enrollment. A newly named primary must first establish
      // its authority through the signed leadership chain. A legacy Mac that changed its keys
      // used this same OS account and a signed replacement roster to update its old enrollment.
      const same = mac === setup.primary && input.macSign === setup.sign && (!offeredSetup || offeredSetup.id === setup.id);
      const legacyRotation = !setup.managed && !usesAcceptedSaves(setup) && replaced.includes(setup.primary)
        && macsOf(current).includes(setup.primary) && offeredSetup?.primary === mac;
      if (!same && !legacyRotation) {
        alreadySetUp = true;
        return null;
      }
    }
    // A box in no setup, set up again by the Mac it already has (a 0.7 Mac does each time its phones
    // change): its work, those phones included, is that Mac's. Any other Mac is refused below.
    const sameMac = !!mac && !current?.setup && macsOf(current).includes(mac);
    if (mac && !sameMac && current?.setup?.role !== "member" && (!current?.setup || offeredSetup?.id !== current.setup.id || offeredRoster?.origin !== current.setup.sign)
      && hasSetupWork(current)) {
      alreadySetUp = true;
      return null;
    }
    if (mac && current && (ownKey(current) === mac
      || (current.setup?.role === "primary" && peersOf(current).length > 0)
      || (macsOf(current).length > 0 && !macsOf(current).some(key => key === mac || replaced.includes(key))))) {
      alreadySetUp = true;
      return null;
    }
    const key = current && ownKey(current);
    if (key && Array.isArray(input.blockedMemberships) && input.blockedMemberships.includes(key)) {
      blockedMembership = true;
      return null;
    }
    if (mac && current && unlinkCleanupLayoutReady(K)) {
      const offered = offeredPhones && takesPhoneScope(current, offeredPhones.origin, offeredPhones.body.setup, offeredSetup?.id ?? current.setup?.id)
        ? offeredPhones.body.phones : undefined;
      const revoked = offeredPhones?.body.removed;
      const matches = (entry: unknown, key: string, pairing: string) => isObject(entry) && entry.key === key && (entry.pairing ?? "") === pairing;
      const unmatched = [...(current.phones ?? []), ...(current.preJoinPairings ?? [])].some(phone => phone.from === "pair" &&
        !(Array.isArray(offered) && offered.some(entry => matches(entry, phone.key, phone.pairing ?? "")) &&
          !(Array.isArray(revoked) && revoked.some(entry => matches(entry, phone.key, phone.pairing ?? "")))));
      if (unmatched && input.discardLocalPairings !== true) {
        blockedPairings = true;
        return null;
      }
      // The incoming signed snapshot supplies grants with the same existing pairing IDs.
      current.phones = current.phones?.filter(phone => phone.from !== "pair");
      delete current.preJoinPairings;
    }
    const sign = current && ownSign(current);
    // SSH authorizes an enrollment. Only a key pair still on the primary's roster
    // may retain its credentials and link grants across this operation.
    const fresh = mac && (revocations.has(key) || Boolean(key && current?.departedKey === key)
      || (Array.isArray(input.keepMemberships)
        ? !input.keepMemberships.some(entry => isObject(entry) && entry.key === key && entry.sign === sign)
        : Boolean(current && !macsOf(current).length && current.setup?.role !== "member")));
    if (mac && current && current.setup?.role !== "member" && !macsOf(current).length) {
      // Old readers could leave an unscoped projection when they detached. Preserve
      // those exact denials as local history before installing the incoming setup.
      archiveDeviceRevocations(current, "unattributed");
    } else if (fresh && current) {
      archiveDeviceRevocations(current);
    }
    if (mac && current && current.setup?.role !== "member" && current.installerClaim?.recovery) {
      archiveInstallerCancellation(current);
    }
    const next = (fresh && current ? { name: current.name, startsFromOthers: current.startsFromOthers,
      phones: [], formerDeviceRevocations: current.formerDeviceRevocations, formerPhoneIssuers: current.formerPhoneIssuers,
      installerCancellations: current.installerCancellations } as LinkState : current) ?? { secret: crypto.randomBytes(32).toString("base64url"), key: newKey().toString("base64url"), phones: [] };
    next.secret ??= crypto.randomBytes(32).toString("base64url");
    next.key ??= newKey().toString("base64url");
    next.sign ??= newSigningSeed();
    if (mac) {
      next.mac = mac;
      next.macs = [...new Set([...macsOf(next), mac])];
      forgetOldMacs(next, mac);
      if (typeof input.macName === "string" && plainName(input.macName)) next.macNames = { ...(next.macNames ?? {}), [mac]: plainName(input.macName).slice(0, 100) };
      // The key its documents are signed with (over SSH, so this box knows it's that Mac's), which
      // takes the place of a list it gave before it signed them.
      if (typeof input.macSign === "string" && SIGN.test(input.macSign)) {
        next.macSigns = { ...(next.macSigns ?? {}), [mac]: input.macSign };
        if (next.legacy) delete next.legacy[mac];
      } else if (Array.isArray(input.peers)) {
        // COMPAT(beta.1 peers-list): the list of peers a beta.1 Mac's setup gives. Goes on the user's word, once no Mac from before 1.0 connects.
        // A Mac that signs none yet (0.8.0-beta.1): its peers as it lists them.
        takeLegacyPeers(next, mac, input.peers, typeof input.peersAt === "number" ? input.peersAt : (typeof input.at === "number" ? input.at : 0), input.moved);
      }
      // That Mac under its keys before: no Mac of this box's any more (never two of one Mac).
      formerMacs = replaced.filter(key => macsOf(next).includes(key));
      for (const key of formerMacs) leaveMac(next, key);
    }
    if (typeof input.id === "string") next.id = input.id.slice(0, 200);
    // COMPAT(0.7 visits): a Mac that doesn't say it `tolerates` newer boxes holds updates back. Goes on the user's word, once no Mac from before 1.0 connects.
    // Whether this Mac leaves a newer box as it is, by its ID (as its engine launches name it): the
    // box updates itself only while every Mac that started its engine lately does (PROTOCOL.md, "Updates").
    if (typeof input.macID === "string" && /^[A-Za-z0-9-]{1,100}$/.test(input.macID)) next.tolerantIDs = { ...(next.tolerantIDs ?? {}), [input.macID]: input.tolerates === 1 };
    // Its documents, and those it carries of your other computers: taken as any are.
    const before = readPersonalities();
    const state = { link: next, library: before };
    // (The Mac setting it up wants it: a removal it signed before never counts here now.)
    docsWork(state, { key: mac ?? "", sign: mac ? macSign(next, mac) : undefined }, { give: Array.isArray(input.docs) ? input.docs : [] }, { stay: mac ?? null });
    if (state.library !== before) updatedLibrary = state.library;
    // (docsWork has folded its phones, so there's a list.)
    if (!next.phones!.length && !peersOf(next).length) return current ? next : null;
    next.relay = String(input.relay);
    next.name = String(input.name || next.name || os.hostname()).slice(0, 100);
    link = next;
    return next;
  });
  if (blockedPairings) {
    console.error("Confirm discarding unmatched local device pairings before joining.");
    process.exit(SETUP_LOCAL_PAIRINGS);
  }
  if (blockedMembership) {
    console.error("Wait for the pending wipe before adding this computer again.");
    process.exit(1);
  }
  if (alreadySetUp) {
    console.error("This OS account already has a Kuvo setup. Connect to your own OS account.");
    process.exit(SETUP_OCCUPIED);
  }
  if (updatedLibrary) await takePersonalities(updatedLibrary).catch(() => {});
  // (What those keys' agents were, told here, goes with them.)
  const after = link ?? readLink();
  if (after) for (const key of formerMacs) forgetMacsAgents(key, after);
  // The running bridge tells your other computers what changed; its setup card says what it's in now.
  noteDocsChanged();
  try { writeCard(); } catch {}
  // None paired any more (each removed for good, as one at a time would be): no bridge.
  if (!link) {
    if (running) process.kill(running, "SIGTERM");
    forgetPhones();
    console.log("{}");
    return;
  }
  // A new relay or name needs a new connection, and a newer bridge replaces an older one.
  let runningVersion = null;
  try { runningVersion = fs.readFileSync(VERSION_FILE, "utf8").trim(); } catch {}
  const restart = old?.key !== link.key || old?.relay !== link.relay || old?.name !== link.name || runningVersion !== BRIDGE_VERSION;
  // Not while the box updates itself: the bridge running it would end, and the keeper starts the
  // new one once it's done.
  if (!isUpdating()) {
    if (running && restart) process.kill(running, "SIGTERM");
    if ((!running || restart) && !installedWslTask()) startBridge();
  }
  console.log(JSON.stringify({
    address: addressOf(Buffer.from(link.secret!, "base64url")).toString("base64url"),
    key: publicOf(Buffer.from(link.key!, "base64url")).toString("base64url"),
    // Talks to other boxes itself (PROTOCOL.md, "Peers").
    peers: true,
    layout: KUVO_FOLDER_LAYOUT,
    // Its signing key, and the documents it has, for the Mac to take (PROTOCOL.md, "Documents").
    sign: ownSign(link),
    docs: setupDocs(link),
  }));
}

// What setup gives back of documents: those this box keeps, and its own as they are now.
function setupDocs(link: LinkState): SignedDoc[] {
  const state = { link, library: readPersonalities() };
  const own = ownDocs(state);
  return [...Object.values(link.docs ?? {}), ...Object.values(own).map(each => each.envelope)];
}

// `leave <mac>`: Mac `mac` removed this machine. It's no longer one of its Macs: its lists of
// peers and phones no longer count here, and what peers only it named told goes. Another Mac's
// stay; with no peers or phones left, the bridge stops.
export function leave(mac: unknown): boolean {
  if (typeof mac !== "string" || !KEY.test(mac)) return false;
  const link = updateLink(link => {
    if (!link || link.setup?.primary === mac || !macsOf(link).includes(mac)) return null;
    leaveMac(link, mac);
    pruneDocs(link);
    foldPhones(link);
    return link;
  });
  if (!link) return false;
  forgetMacsAgents(mac, link);
  noteDocsChanged();
  try { writeCard(); } catch {}
  return true;
}

async function pair(relay: string | undefined): Promise<void> {
  if (relay && !IS_MAC && !readLink()?.relay) updateLink(current => {
    const link = current ?? { secret: crypto.randomBytes(32).toString("base64url"), key: newKey().toString("base64url"), name: os.hostname(), phones: [] };
    link.sign ??= newSigningSeed();
    link.relay = relay;
    settleOwnSetup(link);
    return link;
  });
  await ensurePrimaryLease();
  // Only your setup's main computer pairs phones: a member names it, and pairs none (exit 3).
  let main: ReturnType<typeof mainElsewhere> = null;
  const link = updateLink(current => {
    if (primaryFenced(current)) throw new Error(LEADERSHIP_WAIT);
    main = mainElsewhere(current);
    if (main) return null;
    const next = current ?? { secret: crypto.randomBytes(32).toString("base64url"), key: newKey().toString("base64url"), name: os.hostname(), phones: [] };
    next.sign ??= newSigningSeed();
    if (relay) next.relay = relay;
    if (!next.relay) return null;
    if (!IS_MAC) settleOwnSetup(next);
    next.pairing = newPairingToken();
    return next;
  });
  const primary = main as ReturnType<typeof mainElsewhere>;
  if (primary) {
    console.error(pairElsewhere(primary).message);
    process.exit(3);
  }
  if (!link) {
    console.error("Give the relay's address: kuvo-link.js pair wss://…");
    process.exit(1);
  }
  console.log(pairingLink(link as Parameters<typeof pairingLink>[0]));
  console.error(`Open this link on your phone within ${PAIRING_MINUTES} minutes. It works once.`);
  if (!runningBridge()) startBridge();
}

// Checks this runtime has what the link needs, both ends of a session and a mail.
export function selftest(): boolean {
  const phone = newKey(), host = newKey();
  const initiator = new IKInitiator(phone, publicOf(host), PROLOGUE);
  const responder = new IKResponder(host, PROLOGUE);
  const said = responder.readMessage1(initiator.writeMessage1(Buffer.from("hello")));
  const { message, transport: hostSide } = responder.writeMessage2(Buffer.from("welcome"));
  const { payload, transport: phoneSide } = initiator.readMessage2(message);
  const echoed = hostSide.open(phoneSide.seal(Buffer.from("ping")));
  const mail = openOneWay(sealOneWay(Buffer.from("mail"), host, publicOf(phone)).message, phone, publicOf(host)).payload;
  const ok = said.toString() === "hello" && payload.toString() === "welcome" && echoed.toString() === "ping" && mail.toString() === "mail"
    && responder.rs.equals(publicOf(phone)) && typeof WebSocket === "function" && typeof fetch === "function";
  return ok;
}

const main = process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url));
if (main && process.argv[2] === "box" && process.argv[3] === "tool-command-worker") {
  // The worker only publishes its own receipt and must outlive the installed bundle it began with.
  await box("tool-command-worker", process.argv.slice(4), false);
} else if (main && !isReceiptWorker && !isDiskWorker) {
  const [mode, argument, ...commandArgs] = process.argv.slice(2);
  const updateProbe = mode === "box" && argument === "route" && commandArgs[0] === "--internal" && commandArgs[1] === "kuvo/info";
  const useSshPipe = mode === "box" && (argument === "route" || argument === "route-to") && !updateProbe && sshPipeSelected();
  try {
    // The installer and move own the destination until placement. A probe must not adopt it.
    // The judge only advances the folder when it confirms the trial. It must also be able to
    // resume a rollback whose intent was already saved.
    if (mode === "update") checkRollbackRecords();
    if (mode === "helper" && argument === "hand-over") readHandOver();
    if (mode === "engine-swap" || (mode === "helper" && argument === "machine-start")) {
      readMachineJob(RUN);
      readSwap(RUN);
      readSwap(RUN, installHoldFile(RUN));
    }
    if (["here", "backup-identity", "install", "rehome", "uninstall", "login-path", "login-env"].includes(mode)
      || ["setup", "join", "repair", "machine-job", "supervision-record", "serve"].includes(mode)
      || (mode === "box" && ["labels", "adopt", "seed"].includes(argument))
      || mode === "update") checkFolderVersion(K);
    // A staged probe must leave the current box layout alone.
    else if (mode !== "selftest" && mode !== "release-switch-quiet" && !updateProbe && !useSshPipe) upgradeLinkFolder();
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (mode === "box" && argument === "route") {
      const inaccessible = ["EACCES", "EPERM", "ENOENT", "ENOTDIR", "EROFS"].includes((error as NodeJS.ErrnoException)?.code ?? "");
      const refusal = error instanceof UnsupportedFormatVersion
        ? unsupportedFormatReply(error)
        : { status: inaccessible ? 503 : 500, body: JSON.stringify({ message }) };
      const reply = { status: refusal.status, routeVersion: ROUTE_VERSION, body: refusal.body };
      await new Promise<void>(resolve => process.stdout.write(JSON.stringify(reply) + "\n", () => resolve()));
      process.exit(0);
    }
    console.error(message);
    process.exit(1);
  }
  if (mode === "folder-upgrade") process.exit(0);
  else if (mode === "run") { if (residentInstalled()) keepKeeper(); else await run(); }
  else if (mode === "setup") await setup();
  else if (mode === "setup-local") initializeMachineSetup();
  else if (mode === "pair") await pair(argument);
  else if (mode === "box") await box(argument, commandArgs, useSshPipe);
  else if (mode === "version-layout") {
    if (!activeBin(K) && !updateOnTrial() && !isUpdating()) {
      const version = boxVersion();
      if (version && takeUpdateLock()) {
        try { await convertBin(K, version, installedPack() ?? undefined); }
        finally { fs.rmSync(UPDATE_LOCK, { recursive: true, force: true }); }
      }
    }
  }
  else if (mode === "release-switch-quiet") {
    if (!await releaseSwitchQuiet(argument === "keep")) process.exitCode = 1;
  }
  else if (mode === "continue-update-turns") {
    await finishReleaseHold();
    await continueCutTurns(K);
  }
  else if (mode === "serve") {
    try { await serve(); }
    catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exit(1);
    }
  }
  else if (mode === "box-services") await runBoxServices();
  else if (mode === "leave") {
    const before = readLink();
    if (!before || typeof argument !== "string" || !KEY.test(argument)) {
      console.error("Kuvo Code couldn’t confirm which setup this computer should leave.");
      process.exit(1);
    }
    if (before.setup?.primary === argument) {
      console.error(PRIMARY_UNLINK);
      process.exit(1);
    }
    const name = macName(before, argument);
    if (macsOf(before).includes(argument)) try { leave(argument); } catch (error) { logCleanup(error); }
    const after = readLink();
    if (!after || macsOf(after).includes(argument)) {
      console.error("Kuvo Code couldn’t confirm that this computer left the setup. Run the command again.");
      process.exit(1);
    }
    console.error(`${name || "That Mac"} is let go: this computer is no longer one of its computers.`);
  }
  // Kuvo Code as it opens on a Mac, before it starts a box (PROTOCOL.md, "Your setup", "Already in a
  // setup?"): whether it uses its own box, or the one already on this Mac, in place. `here <root>`,
  // root the machines' folder's name in the home folder (`.kuvo-code`, or a test's own), with
  // KUVO_HOME the app's own box's folder: both said, never guessed.
  else if (mode === "backup-identity") {
    const key = ownKey(readLink() ?? {});
    if (key) console.log(key);
    else process.exitCode = 1;
  }
  else if (mode === "here") {
    try {
      console.log(JSON.stringify(hereNow(argument)));
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exit(2);
    }
  }
  // A machine's start scripts' (`kuvo-launch`): what its engine starts with, its OPENCODE_CONFIG_CONTENT
  // with the sandbox's note, and what some providers need beside its settings, as this box last
  // learned it (PROTOCOL.md, "Settings per box").
  else if (mode === "engine-config") {
    // A machine's start runs this before each start of its engine, with or without a bridge or a Mac:
    // its settings as its catalogs have them, before the engine reads them. (This Mac's app runs it
    // too, for what it prints, not as its box: `serve` did this, and there's no settings file here.)
    await upgradeIntegrations().catch(() => {});
    // And what's left of its bots' own keys gone, before its engine reads their folders' settings
    // (bots/own-keys.ts): a machine without a bridge does it here.
    if (!IS_MAC) await dropBotsOwnKeys().catch(() => {});
    // What it learned before it kept it there (an older box's), or a copy taken away meanwhile.
    if (knowsEngineSettings()) {
      try { applyCompatibility(); } catch {}
    }
    // `engine-config tools`, from the box's own start (`kuvo-launch`, a Mac's own box's too): with
    // Kuvo Code's own tools for its engine, run with this same runtime, the engine's. (Not for the
    // retired Mac app's box in its folder, whose app gives its engine its own.)
    // (Whole, as the start reads it to its end: never cut short by an exit as it goes out.)
    sayWhole(engineConfigContent(process.env.OPENCODE_CONFIG_CONTENT, argument === "tools" ? { runtime: process.execPath } : undefined));
  }
  // A machine's start scripts' too: the PATH your login shell sets up there (`~/.local/bin`, nvm's
  // node), then the folders of the one it was run with, for the engine and what it starts, which a
  // start over SSH or from cron doesn't get. Nothing when the shell doesn't give it, for the start
  // to keep the PATH it has.
  // Kept for the link and the agents' bridge (each read can leave a program a .bashrc starts running).
  else if (mode === "login-path") {
    const found = await loginPath();
    if (found) saveLoginPath(K, found);
    sayWhole(found ?? "");
  }
  // A Mac's own box's start (`kuvo-launch`): your login shell's whole environment, its PATH kept as
  // `login-path` keeps it, for the engine and what it starts (launchd gives Kuvo Helper, and so the
  // box, a bare one). Nothing when the shell doesn't give it, for the start to read its PATH alone.
  else if (mode === "login-env") {
    const found = await loginEnvironment();
    if (found?.PATH) saveLoginPath(K, found.PATH);
    process.stdout.write(found?.PATH ? launchFromLogin(found) : "");
  }
  // The update on trial judged (`update --settle`), as the keeper starts it when nothing judges it
  // (its updater went first: the machine went down, or an update from before this rule).
  else if (mode === "update" && process.argv.includes("--settle")) {
    console.log(JSON.stringify({ settled: await settleUpdate({ well: updateIsWell }) }));
  }
  // A newer engine than the one at work, put in place by an update (the keeper's `V`): changed at a
  // quiet moment, what's sent meanwhile held on the box (`engine-swap`, as the keeper starts it;
  // updates/engine-swap.ts). Stays until it's done.
  else if (mode === "supervision-record") {
    try {
      if (argument === "bridge") {
        const record = readBridgeFile();
        console.log("v1", Number.isSafeInteger(record.pid) ? record.pid : 0, /^[0-9a-f]{64}$/.test(record.build ?? "") ? record.build : "-");
      } else if (argument === "bridge-http") {
        const record = readBridgeFile();
        if (!Number.isSafeInteger(record.port) || record.port <= 0 || record.port > 65535
            || typeof record.password !== "string" || !record.password || /[\r\n]/.test(record.password)) {
          throw new Error("The bridge HTTP rendezvous is unreadable.");
        }
        console.log(`v1\n${record.port}\n${record.password}`);
      } else if (argument === "engine-swap") {
        console.log("v1", readSwap(RUN)?.pid ?? 0);
      } else if (argument === "rollback") {
        checkRollbackRecords();
        console.log("v1");
      } else throw new Error("Unknown supervision record.");
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    }
  }
  else if (mode === "machine-job") {
    try {
      const job = readMachineJob(RUN);
      if (job) console.log("v1", job.phase, job.owner, job.legacy ? "legacy" : "fresh");
      else console.log("v1 absent");
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    }
  }
  else if (mode === "engine-swap") {
    if (argument === "--machine-helper-recover") await recoverMachineJob();
    else if (argument !== "--machine-helper" || await handOverMachine() === "deferred") {
      console.log(JSON.stringify({ swapped: await swapEngine() }));
    }
  }
  // Kuvo's own programs here put back from the signed release this box runs, where one isn't what it
  // says (`repair`, as the keeper starts it: updates/repair.ts). A test's folder only takes a test's
  // key (`KUVO_TEST_RELEASE_KEY`, from its launch's environment).
  else if (mode === "repair") {
    try {
      checkRepairRequest(process.argv.slice(3));
      checkRepairFormat();
      upgradeLinkFolder();
      const key = process.env.KUVO_TEST_RELEASE_KEY, feed = process.env.KUVO_TEST_RELEASE_FEED;
      const repair = await repairWithFetch({ ...(key && !REAL_BOX ? { keys: [key] } : {}), ...(feed && !REAL_BOX ? { feed } : {}) });
      console.log(JSON.stringify(repairReplyFormat.write(repair)));
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exit(1);
    }
    // (A look, once: nothing it fetched with keeps it running, nor the keeper's next look waiting.)
    process.exit(0);
  }
  else if (mode === "update") {
    const args = argument === "--switch" ? commandArgs : process.argv.slice(3);
    const option = (name: string): string | undefined => (args.indexOf(name) >= 0 ? args[args.indexOf(name) + 1] : undefined);
    const from = option("--from"), phase = option("--phase"), version = option("--version"), sha256 = option("--sha256");
    const expected = version && versionOrder(version) && sha256 && /^[a-f0-9]{64}$/.test(sha256) ? { version, sha256 } : undefined;
    const test = option("--key") && !REAL_BOX ? { url: option("--url"), keys: [option("--key")!], blocked: null } : {};
    if (test.keys) fleetWith(test);
    const outcome = from && !path.isAbsolute(from)
      ? { result: "failed" as const, message: "Send the full path to the release folder." }
      : await update({ now: args.includes("--now"), from, phase: phase === "prepare" || phase === "activate" ? phase : undefined,
        expected, step: running => noteUpdate({ running }), ...test });
    if (outcome.message !== UPDATING_ALREADY) noteUpdate({ running: null, last: { at: Date.now(), ...outcome } });
    const round = option("--round");
    if (round && outcome.result !== "busy") noteUpdate({ round });
    console.log(JSON.stringify(outcome));
  }
  // The browser agents use here (PROTOCOL.md, "The browser on a machine"): `browser install [--now]
  // [--repair]` sets it up, as the keeper starts it (one at a time); `browser start [--now]
  // [--repair]` has it set up in a process of its own, as a browser tool does, and says how it
  // stands then; `browser status` says how it stands; `browser fold` takes in the pages its keeper
  // noted for the history ("The browser's data"), as the keeper asks once they've piled up; `browser
  // take-in`, what this Mac's app's panel kept, taken in once, as the keeper first starts.
  else if (mode === "browser") {
    const args = process.argv.slice(3);
    if (argument === "install") {
      const outcome = await installBrowser({ now: args.includes("--now"), repair: args.includes("--repair") });
      console.log(JSON.stringify({ outcome, ...browserStatus() }));
    } else if (argument === "start") {
      console.log(JSON.stringify(startBrowserInstall({ now: args.includes("--now"), repair: args.includes("--repair") })));
    } else if (argument === "status") {
      console.log(JSON.stringify(browserStatus()));
    } else if (argument === "fold") {
      foldHistory();
      console.log(JSON.stringify({ folded: true }));
    } else if (argument === "take-in") {
      takeInAppBrowserData(IS_MAC ? thisMacsAppFolder() : null);
      console.log(JSON.stringify({ taken: true }));
    } else {
      console.error("Usage: kuvo-link.js browser install|start [--now] [--repair] | status | fold | take-in");
      process.exit(2);
    }
  }
  // Moving this Mac's box out of its app's folder into a home of its own (PROTOCOL.md, "Moving this
  // Mac's box"), and back: `rehome <root> [--ready | --resume | --back | --status | --settled <what>]`.
  else if (mode === "rehome") {
    process.exitCode = await rehomeCommand(argument, process.argv.slice(4));
  }
  // This computer's box installed from the release an app carries (PROTOCOL.md, "Installing this
  // computer's box"): `install --status | --new | --move`, which `kuvo-install` hands over to.
  else if (mode === "install") {
    process.exitCode = await installCommand(process.argv.slice(3));
  }
  else if (mode === "join") {
    try { console.log(JSON.stringify(await enrollInstalled(await installerCode(), upgradeLinkFolder))); }
    catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    }
  }
  else if (mode === "installer-recovery") {
    try {
      if (argument === "issue") console.log(installerCancellationReceipts().join("\n"));
      else if (argument === "apply") {
        const server = runningServe();
        if (!server || !Number.isSafeInteger(server.port) || server.port <= 0) throw new Error("Start Kuvo on the main computer and try again.");
        const receipt = await installerCode("Enter the installer cancellation receipt");
        const response = await fetch(`http://127.0.0.1:${server.port}/${INSTALLER_RECOVERY_ROUTE}`, {
          method: "POST", redirect: "error", signal: AbortSignal.timeout(10_000),
          headers: { authorization: `Basic ${Buffer.from(`opencode:${server.password}`).toString("base64")}`,
            "content-type": "application/json", "x-kuvo-from": IS_MAC ? "app" : "mac" },
          body: JSON.stringify({ receipt: receipt.trim() }),
        });
        const { message } = await response.json() as { message: string };
        if (!response.ok) throw new Error(message || "The roster retirement could not be confirmed.");
        console.log(message);
      } else throw new Error("Use installer-recovery issue or installer-recovery apply.");
    } catch (error) {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    }
  }
  else if (mode === "wipe-member") {
    process.exitCode = await wipeCommand();
  }
  // Legacy uninstall from the app or a Terminal, with its own dry run.
  else if (mode === "uninstall") {
    process.exitCode = await uninstallCommand(process.argv.slice(3));
  }
  // This Mac's box's helper, Kuvo Helper, started at login (PROTOCOL.md, "This Mac's box: its
  // helper"): `helper install [--load] [--app <bundle ID>] | helper start [--if-missing] [--app <bundle ID>] | helper status`.
  // COMPAT(beta.30 helper-handover): `helper hand-over --pid <helper>`, which serve starts. Goes once no box is from before beta.31.
  else if (mode === "helper" && process.argv[3] === "hand-over") {
    process.exitCode = handOverCommand(process.argv.slice(4));
  }
  else if (mode === "helper" && argument === "machine-start") {
    process.exitCode = await startMachineInstall() ? 0 : 1;
  }
  else if (mode === "helper") {
    process.exitCode = helperCommand(process.argv.slice(3));
  }
  else if (mode === "selftest") {
    const ok = selftest();
    console.log(ok ? "ok" : "failed");
    process.exit(ok ? 0 : 1);
  } else {
    console.error("Usage: kuvo-link.js run | serve [--kept|--resident] | setup | pair [relay] | box <command> | leave <mac> | here <root> | rehome <root> | install --status|--new|--move | uninstall [--check] | helper install|start|status | engine-config | login-path | update [--now] | browser install|start|status|fold|take-in | selftest");
    process.exit(2);
  }
}

export { frameBudget, browserView } from "./browser/view";
export { viewUpgrade } from "./browser/websocket";
export { repairFromRelease, repairWithFetch } from "./updates/repair";
