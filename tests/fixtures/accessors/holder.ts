const noop = (value: string): void => {
  void value;
};

export class Holder {
  get handler(): (value: string) => void {
    return noop;
  }
}
