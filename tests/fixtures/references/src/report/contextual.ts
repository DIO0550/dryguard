import { Shape } from "../shared/shape";

// `billing/contextual.ts` と同じ形。共有の `Shape` を受けるので、尋ねられれば重なる
export const traceReport: (figure: Shape) => void = function inner(figure) {
  void figure;
};
