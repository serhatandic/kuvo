// kuvo-keys, the one-time hand-over of Kuvo Code 0.7's Keychain items to this Mac's box
// (`KeysTool`): built and signed as Kuvo Code itself by scripts/build-helper.sh.
#if os(macOS)
import KuvoHelperKit

KeysTool.main(CommandLine.arguments)
#endif
