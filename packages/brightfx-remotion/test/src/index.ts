// The instrumentation first, so it is in place before any composition loads.
import "./probes";
import { registerRoot } from "remotion";
import { Root } from "./Root";

registerRoot(Root);
