import { Shape } from "../shared/shape";

// 引数の型は代入先の注釈（`Shape`）から来る。**注釈はソースに書かれている**
export const traceBilling: (figure: Shape) => void = function inner(figure) {
  void figure;
};
