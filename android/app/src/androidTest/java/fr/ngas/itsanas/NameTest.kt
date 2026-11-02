package fr.ngas.itsanas

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/** Scenario 2: the username, checked against a real coordinator as it is typed. */
@RunWith(AndroidJUnit4::class)
class NameTest {
    @Before fun before() = E2e.begin()

    @After fun after() = E2e.end()

    @Test
    fun a_name_with_an_underscore_is_refused_in_words_and_next_stays_put() {
        Flows.toAccountStep()
        Flows.newAccountNamed("Marie_1")
        val said = E2e.textOf("#username-check")
        E2e.shotBoth("03-name-refused")
        assertTrue("`Marie_1` was not refused in words, so the person is not told what is wrong: '$said'", said.isNotEmpty() && !said.contains("free"))
        Flows.nextOnAccount()
        E2e.pause(1500)
        assertTrue(
            "Next went on with a name the coordinator would refuse, so setup would fail minutes later",
            E2e.onSection("account") && E2e.visible("#problem"),
        )
    }

    @Test
    fun a_free_name_is_accepted_and_next_goes_on() {
        Flows.toAccountStep()
        Flows.newAccountNamed(Flows.freshName())
        E2e.waitFor("a free name was not called free") { E2e.textOf("#username-check") == "This username is free." }
        E2e.shotBoth("04-name-free")
        Flows.nextOnAccount()
        E2e.waitFor("Next did nothing with a free name, so nobody can create an account") { E2e.onSection("folder") }
        E2e.shotBoth("05-space")
    }

    @Test
    fun a_coordinator_that_cannot_be_reached_gives_no_next_and_a_try_again() {
        // Nothing listens on port 1 of the host: the check cannot be answered.
        Native.useCoordinatorForTests("10.0.2.2:1", "")
        E2e.relaunch()
        Flows.toAccountStep()
        Flows.newAccountNamed(Flows.freshName())
        E2e.waitFor("no Try again was offered when the network could not be asked") { E2e.visible("#username-retry") }
        E2e.shotBoth("06-name-no-network")
        Flows.nextOnAccount()
        E2e.pause(1500)
        assertTrue(
            "Next went on although the network never said the name was free, so two people could claim one name",
            E2e.onSection("account"),
        )
    }
}
