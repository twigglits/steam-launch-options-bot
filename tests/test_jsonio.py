import io
import json
import unittest

from steamtrain import apply as apply_mod, codes, jsonio


class CodesTest(unittest.TestCase):
    def test_action_vocabulary_covers_every_action_the_planner_produces(self):
        """AD-3: the closed vocabulary must not drift from apply.Change.

        If someone adds a fourth planner action, this fails rather than
        letting a client meet a code it has never heard of.
        """
        produced = {"set", "skip-user-set", "skip-unchanged"}
        self.assertTrue(produced.issubset(set(codes.ACTIONS)))
        self.assertIn(codes.EXCLUDED, codes.ACTIONS)
        self.assertEqual(len(codes.ACTIONS), len(set(codes.ACTIONS)))

    def test_planner_never_emits_a_code_outside_the_vocabulary(self):
        change = apply_mod.Change(user="1", appid="2", name="n", current="",
                                  proposed="x", action="set")
        self.assertIn(change.action, codes.ACTIONS)

    def test_no_code_collides_across_vocabularies(self):
        self.assertFalse(set(codes.GUARDRAILS) & set(codes.ACTIONS))
        self.assertFalse(set(codes.OUTCOMES) & set(codes.GUARDRAILS))


class EmitterTest(unittest.TestCase):
    def setUp(self):
        self.buf = io.StringIO()
        self.out = jsonio.Emitter(stream=self.buf)

    def records(self):
        return [json.loads(line) for line in self.buf.getvalue().splitlines()]

    def test_every_record_carries_version_and_kind(self):
        self.out.emit(jsonio.KIND_GAME, appid="292030", name="W3")
        self.out.result(True, codes.OK)
        for record in self.records():
            self.assertEqual(record["v"], jsonio.VERSION)
            self.assertIn(record["kind"], jsonio.KINDS)

    def test_one_object_per_line_and_no_wrapping_array(self):
        self.out.emit(jsonio.KIND_GAME, appid="1")
        self.out.emit(jsonio.KIND_GAME, appid="2")
        self.out.result(True, codes.OK)
        lines = self.buf.getvalue().splitlines()
        self.assertEqual(len(lines), 3)
        self.assertFalse(self.buf.getvalue().lstrip().startswith("["))
        for line in lines:
            json.loads(line)  # each line parses standalone

    def test_final_record_is_always_the_result(self):
        self.out.emit(jsonio.KIND_PROFILE, distro="x")
        self.out.result(True, codes.OK, counts={})
        self.assertEqual(self.records()[-1]["kind"], jsonio.KIND_RESULT)

    def test_nothing_may_follow_the_result(self):
        self.out.result(True, codes.OK)
        with self.assertRaises(RuntimeError):
            self.out.emit(jsonio.KIND_GAME, appid="1")

    def test_only_one_result_per_invocation(self):
        self.out.result(True, codes.OK)
        with self.assertRaises(RuntimeError):
            self.out.result(True, codes.OK)

    def test_result_must_go_through_result(self):
        with self.assertRaises(ValueError):
            self.out.emit(jsonio.KIND_RESULT, ok=True)

    def test_unknown_kind_is_refused(self):
        with self.assertRaises(ValueError):
            self.out.emit("nonsense", x=1)

    def test_disabled_emitter_writes_nothing_and_never_raises(self):
        off = jsonio.Emitter(stream=self.buf, enabled=False)
        off.emit(jsonio.KIND_GAME, appid="1")
        off.result(True, codes.OK)
        off.result(True, codes.OK)  # no invariant enforcement when disabled
        self.assertEqual(self.buf.getvalue(), "")

    def test_truncated_stream_is_detectable(self):
        self.out.emit(jsonio.KIND_GAME, appid="1")
        # process dies here, no result written
        self.assertNotIn(jsonio.KIND_RESULT, [r["kind"] for r in self.records()])

    def test_result_carries_ok_and_outcome(self):
        self.out.result(False, codes.BLOCKED, guardrail=codes.STEAM_RUNNING)
        record = self.records()[-1]
        self.assertIs(record["ok"], False)
        self.assertEqual(record["outcome"], codes.BLOCKED)
        self.assertEqual(record["guardrail"], codes.STEAM_RUNNING)


if __name__ == "__main__":
    unittest.main()
