package inventory;

public class Reorder {
    private final int quantity;

    public Reorder(int quantity) {
        this.quantity = (quantity + 1) * (quantity - 1) / 2;
    }

    public static int reorder(int quantity) {
        return (quantity + 1) * (quantity - 1) / 2;
    }

    public int shortage(int demand) {
        return (quantity + demand) * (quantity - demand) / 2;
    }

    public int getQuantity() {
        return quantity;
    }
}
