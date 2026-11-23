// The browser's data on a box (PROTOCOL.md, "The browser"): its history and site icons, the
// sites that keep data in it, and its settings. Every box keeps its own, in its `browser` folder,
// and they never leave it but in these answers.

import { BYTES, bool, type Infer, list, number, object, type RouteShape, string, words } from "../shape";

/** The search engines the browser offers, by id, with their names and their addresses (`%s` for the
 * words): what you type that isn't an address searches there. */
export const SEARCH_ENGINES = [
  { id: "google", title: "Google", url: "https://www.google.com/search?q=%s" },
  { id: "duckDuckGo", title: "DuckDuckGo", url: "https://duckduckgo.com/?q=%s" },
  { id: "bing", title: "Bing", url: "https://www.bing.com/search?q=%s" },
  { id: "brave", title: "Brave Search", url: "https://search.brave.com/search?q=%s" },
  { id: "kagi", title: "Kagi", url: "https://kagi.com/search?q=%s" },
  { id: "ecosia", title: "Ecosia", url: "https://www.ecosia.org/search?q=%s" },
] as const;

export const SEARCH_ENGINE = words("google", "duckDuckGo", "bing", "brave", "kagi", "ecosia");

/** A site in the history: its key (`host`, without `www.`, with a port where the address names one),
 * the address that names it best (its front page's, once seen) and that page's title, how many
 * times it was opened, when last (ms since 1970), and whether the box keeps its icon. */
export const HISTORY_SITE = object({ host: string, url: string, title: string, visits: number, lastVisit: number, icon: bool });

/** The history, most suggested first: visits count, and fade over a few weeks (a new tab page shows
 * the first few). */
export const BROWSER_HISTORY = object({ sites: list(HISTORY_SITE) });

/** A site that keeps data in the browser (its registrable domain, as `example.co.uk`), and what it
 * keeps: `cookies` (sign-ins among them), `storage` (what pages store: databases, local storage). */
export const DATA_SITE = object({ site: string, kinds: list(words("cookies", "storage")) });

/** The sites that keep data in the browser, by name. */
export const BROWSER_SITES = object({ sites: list(DATA_SITE) });

/** The browser's settings: its search engine (and those it offers, in order, each with its name);
 * whether agents' tabs no one has looked at close an hour after their last use; whether it offers
 * to save a password you sign in with, and the sites you said Never to. */
export const BROWSER_SETTINGS = object({
  searchEngine: SEARCH_ENGINE,
  searchEngines: list(object({ id: SEARCH_ENGINE, title: string })),
  tidiesAgentTabs: bool,
  offersToSave: bool,
  never: list(string),
});

export type SearchEngine = Infer<typeof SEARCH_ENGINE>;
export type HistorySite = Infer<typeof HISTORY_SITE>;
export type BrowserHistory = Infer<typeof BROWSER_HISTORY>;
export type DataSite = Infer<typeof DATA_SITE>;
export type BrowserSites = Infer<typeof BROWSER_SITES>;
export type BrowserSettings = Infer<typeof BROWSER_SETTINGS>;

export const BROWSER_ROUTES: RouteShape[] = [
  // The browser's live view, opened as a socket (101); 426 for a plain request.
  { route: "GET kuvo/browser/view", feature: "browser.view", answers: { 101: BYTES, 426: object({ message: string }) } },
  // The sites the browser opened, most suggested first.
  { route: "GET kuvo/browser/history", feature: "browser.data", answers: { 200: BROWSER_HISTORY } },
  // Clear the history, the keeper's notes not taken in yet too.
  { route: "DELETE kuvo/browser/history", feature: "browser.data", answers: { 200: BROWSER_HISTORY } },
  // Take one site out of the history, with its icon, until it's visited again.
  { route: "DELETE kuvo/browser/history/:host", feature: "browser.data", answers: { 200: BROWSER_HISTORY } },
  // A site's icon as its pages gave it, or 404.
  { route: "GET kuvo/browser/icons/:host", feature: "browser.data", answers: { 200: BYTES } },
  // The sites keeping cookies or storage in the browser, by registrable domain.
  { route: "GET kuvo/browser/sites", feature: "browser.data", answers: { 200: BROWSER_SITES } },
  // Remove every site's data, signing the browser out of them.
  { route: "DELETE kuvo/browser/sites", feature: "browser.data", answers: { 200: BROWSER_SITES } },
  // Remove one site's data, signing the browser out of it.
  { route: "DELETE kuvo/browser/sites/:site", feature: "browser.data", answers: { 200: BROWSER_SITES } },
  // Search engine, tidying agents' tabs, offering to save passwords, and the Never sites.
  { route: "GET kuvo/browser/settings", feature: "browser.data", answers: { 200: BROWSER_SETTINGS } },
  // Change any of those settings; `never` replaces the whole list.
  { route: "PATCH kuvo/browser/settings", feature: "browser.data", answers: { 200: BROWSER_SETTINGS } },
  // Add one host to the sites passwords are never saved for.
  { route: "PUT kuvo/browser/settings/never/:host", feature: "browser.data.never", answers: { 200: BROWSER_SETTINGS, 400: object({ message: string }) } },
  // Take one host off the Never list.
  { route: "DELETE kuvo/browser/settings/never/:host", feature: "browser.data.never", answers: { 200: BROWSER_SETTINGS, 400: object({ message: string }) } },
];
