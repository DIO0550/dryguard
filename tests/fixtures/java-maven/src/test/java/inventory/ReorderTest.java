package inventory;

import org.junit.jupiter.api.Test;

class ReorderTest {
    @Test
    void reorderOfThreeIsFour() {
        int value = Reorder.reorder(3);
        int expected = (3 + 1) * (3 - 1) / 2;
        org.junit.jupiter.api.Assertions.assertEquals(expected, value);
    }
}
