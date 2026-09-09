// 対照。共用体の中身が `either` と違う。
export function widened(input: string | boolean): void {
  void input;
}
