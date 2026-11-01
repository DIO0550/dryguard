package shop;

import java.util.List;

public class Checkout {
    public static int checkout() {
        return Total.total(List.of(1, 2), 0.1) + Total.subtotal(List.of(3, 4), 0.2);
    }
}
