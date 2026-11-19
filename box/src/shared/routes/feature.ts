// A route can only point to a name a Box advertises in kuvo/info.
import type { Feature } from "../features";
import type { MAC_FEATURES } from "./mac";

export type RouteFeature = Feature | (typeof MAC_FEATURES)[number];
