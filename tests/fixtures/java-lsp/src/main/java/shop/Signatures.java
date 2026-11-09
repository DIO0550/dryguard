package shop;

public class Signatures {
    private final int base;

    public Signatures(int base) {
        this.base = base;
    }

    public static int added(int left, int right) {
        int sum = left + right;
        return sum * 2;
    }

    public static int summed(int first, int second) {
        int sum = first + second;
        return sum * 2;
    }

    public static double scaled(double left, int right) {
        double sum = left + right;
        return sum * 2;
    }

    public static <T extends Priced> T cheaper(T left, T right) {
        return left.price() <= right.price() ? left : right;
    }

    public static <U extends Priced> U lower(U first, U second) {
        return first.price() <= second.price() ? first : second;
    }

    public static <T extends Named> T earlier(T left, T right) {
        return left.name().compareTo(right.name()) <= 0 ? left : right;
    }

    public int offset(int... values) throws OutOfStock {
        if (values.length == 0) {
            throw new OutOfStock();
        }
        return base + values[0];
    }

    public int shifted(int[] values) throws OutOfStock {
        if (values.length == 0) {
            throw new OutOfStock();
        }
        return base - values[0];
    }

    public int rounded(int... values) {
        if (values.length == 0) {
            return base;
        }
        return base * values[0];
    }
}
