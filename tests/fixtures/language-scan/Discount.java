package billing;

public class Discount {
    public double apply(double price, double rate) {
        double discounted = price * rate;
        return Math.max(discounted, 0.0);
    }
}
