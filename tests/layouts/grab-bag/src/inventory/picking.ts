import { chunk } from "../utils/helpers";
import { Location, onHandAt } from "./location";

const WAVE_SIZE = 20;

export function pickWaves(locations: Location[]): Location[][] {
  const pickable: Location[] = [];
  for (const location of locations) {
    if (onHandAt(location) === 0) {
      continue;
    }
    pickable.push(location);
  }
  return chunk(pickable, WAVE_SIZE);
}
