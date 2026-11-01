package shop;

import java.util.List;

public class Total {
    /** 品目の合計から割引を引く。 */
    public static int total(List<Integer> items, double discount) {
        int sum = items.stream().mapToInt(Integer::intValue).sum();
        return sum - (int) (sum * discount);
    }

    public static int subtotal(List<Integer> items, double rate) {
        int sum = items.stream().mapToInt(Integer::intValue).sum();
        return sum - (int) (sum * rate);
    }
}
