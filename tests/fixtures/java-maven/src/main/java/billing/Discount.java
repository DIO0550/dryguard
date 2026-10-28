package billing;

public class Discount {
    private final int amount;

    public Discount(int amount) {
        this.amount = (amount + 1) * (amount - 1) / 2;
    }

    public static int discount(int amount) {
        return (amount + 1) * (amount - 1) / 2;
    }

    public int total(int rate) {
        return (amount + rate) * (amount - rate) / 2;
    }

    public int getAmount() {
        return amount;
    }
}
