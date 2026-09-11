// 括弧・`as`・非 null を重ねた形。名前の位置を取れないと、サーバは答えられるのに
// 尋ねに行けない。
export const asserted = (((value: string): string => value) as (v: string) => string)!;
