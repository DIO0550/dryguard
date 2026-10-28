package billing;

import org.junit.jupiter.api.Test;

class DiscountTest {
    @Test
    void discountOfThreeIsFour() {
        int value = Discount.discount(3);
        int expected = (3 + 1) * (3 - 1) / 2;
        org.junit.jupiter.api.Assertions.assertEquals(expected, value);
    }
}
