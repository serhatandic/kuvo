// Kuvo Helper, the program a Mac's box runs under in a home of its own (`BoxHelper`): built into
// its bundle by scripts/build-helper.sh.
#if os(macOS)
import KuvoHelperKit

BoxHelper.main(CommandLine.arguments)
#endif
