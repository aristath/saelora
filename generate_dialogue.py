#!/usr/bin/env python3
"""
Generate synthetic dialogue for testing LLM memory and vector search.
100k+ messages simulating ~3 years of daily conversation.
Purely template-based, no external APIs.
"""

import json
import random
from datetime import datetime, timedelta

random.seed(42)

# ============================================================
# WORLD STATE
# ============================================================

class World:
    def __init__(self):
        self.date = datetime(2024, 3, 15)
        self.day = 0

        # People
        self.partner = "Mira"
        self.kid = "Leo"
        self.kid_age = 6
        self.sister = "Dana"
        self.best_friend = "Jake"
        self.work_friend = "Priya"
        self.boss = "Marcus"
        self.neighbor = "Tom"
        self.cat = "Biscuit"
        self.old_friend = "Sam"
        self.sister_partner = "Nate"
        self.new_boss = "Linda"
        self.jake_gf = "Sophia"
        self.leo_teacher = "Ms. Chen"
        self.therapist = "Dr. Reyes"
        self.coworker2 = "Derek"
        self.leo_friend = "Ava"
        self.mira_friend = "Jess"
        self.dad_doctor = "Dr. Patel"

        # Work
        self.company = "Vektor"
        self.new_company = "Meridian"
        self.city = "Portland"
        self.neighborhood = "Sellwood"

        # State
        self.active_arcs = set()
        self.completed_arcs = set()
        self.mood = 0.6  # 0=bad, 1=great

    def month_index(self):
        return (self.date.year - 2024) * 12 + self.date.month - 3

    def season(self):
        m = self.date.month
        if m in (3, 4, 5): return "spring"
        if m in (6, 7, 8): return "summer"
        if m in (9, 10, 11): return "fall"
        return "winter"

    def is_weekend(self):
        return self.date.weekday() >= 5

    def dow(self):
        return self.date.strftime("%A")

    def advance(self):
        self.date += timedelta(days=1)
        self.day += 1
        if self.date.month == 3 and self.date.day == 22:
            self.kid_age += 1


# ============================================================
# ARC DEFINITIONS
# ============================================================

def build_arcs(w):
    """Each arc: (name, start_day, end_day, beats_dict, ripples_list)
    beats_dict: {day_offset: [(role, msg), ...]}
    ripples: [str, ...] - user messages sprinkled between beats
    """
    return [
        # --- Arc 1: Billing rewrite (days 0-55) ---
        ("work_project", 0, 55, {
            3: [
                ("u", "got pulled into a new project at work today"),
                ("u", f"{w.boss} wants the whole thing done in three weeks"),
                ("u", "it's a full rewrite of the billing system"),
                ("u", f"{w.work_friend} is on it with me at least"),
                ("s", "three weeks for a rewrite. how big is the current system?"),
                ("u", "no idea. haven't even opened the repo yet"),
                ("u", "but the fact they want a rewrite says enough"),
            ],
            10: [
                ("u", "been reading the billing code all week"),
                ("u", "whoever wrote this should be in jail"),
                ("u", "zero tests. the schema has a table called misc_stuff"),
                ("s", "how bad is the timeline looking now?"),
                ("u", "bad. told Marcus we need at least five weeks"),
                ("u", "he said he'd push back on the stakeholders"),
            ],
            20: [
                ("u", f"we missed the original deadline. {w.boss} got us two more weeks"),
                ("u", f"{w.work_friend} is handling the frontend, I'm doing all the backend"),
                ("u", "it's a lot but we're making progress"),
                ("s", "are the extra two weeks going to be enough?"),
                ("u", "they'll have to be"),
            ],
            35: [
                ("u", "I've been getting home after nine every night this week"),
                ("u", f"{w.partner} is not happy about it"),
                ("u", "I get it but what am I supposed to do"),
                ("s", "have you told her when it ends?"),
                ("u", "told her two more weeks. she just looked at me"),
            ],
            48: [
                ("u", "we launched the billing rewrite today"),
                ("u", "couple bugs in the first hour but nothing critical"),
                ("u", "honestly just relieved"),
                ("s", "three weeks turned into seven. how do you feel about the result?"),
                ("u", "tired but proud. it's genuinely better now"),
                ("u", f"{w.work_friend} and I are getting drinks Friday to celebrate"),
            ],
        }, [
            "the billing rewrite is eating my life",
            f"another late night because of the project",
            f"{w.work_friend} found another edge case in the billing code",
            f"{w.boss} asked for another status update on the rewrite",
            "can't think about anything except database schemas",
            "the billing thing is almost done I can feel it",
            "I dream about SQL now",
        ]),

        # --- Arc 2: Leo school adjustment (days 0-90) ---
        ("leo_school", 0, 90, {
            5: [
                ("u", f"{w.kid} had a rough day at school"),
                ("u", f"his teacher {w.leo_teacher} said he's having trouble making friends"),
                ("u", "he's always been a bit shy but I thought he'd settle in"),
                ("s", f"how's {w.kid} talking about it at home?"),
                ("u", "he doesn't really. just says school is boring"),
            ],
            18: [
                ("u", f"{w.kid} came home crying today"),
                ("u", "said nobody wanted to play with him at recess"),
                ("u", "broke my heart"),
                ("s", "did he say what happened specifically?"),
                ("u", "just that the other kids already have friend groups"),
                ("u", f"I'm thinking about calling {w.leo_teacher}"),
            ],
            30: [
                ("u", f"talked to {w.leo_teacher} today"),
                ("u", f"she said {w.kid} is actually doing fine academically"),
                ("u", "the social stuff just takes time apparently"),
                ("u", "she's going to pair him with a kid named Ava for a project"),
                ("s", "that sounds like a good step. how did you feel after the call?"),
                ("u", "better. she seems like she actually cares"),
            ],
            55: [
                ("u", f"{w.kid} mentioned {w.leo_friend} for the first time today"),
                ("u", "apparently they've been sitting together at lunch"),
                ("u", "he actually seemed excited about school for once"),
                ("s", f"that's a shift. {w.leo_friend} from the project pairing?"),
                ("u", "yeah. seems like it worked"),
            ],
            80: [
                ("u", f"{w.kid} got invited to {w.leo_friend}'s birthday party"),
                ("u", "he's SO excited"),
                ("u", f"asked {w.partner} to help him pick out a present"),
                ("s", "long way from nobody playing with him at recess"),
                ("u", "I know. makes me want to cry for different reasons now"),
            ],
        }, [
            f"{w.kid} didn't want to go to school this morning",
            f"{w.kid} had homework for the first time. he was not thrilled",
            f"dropped {w.kid} off at school. he looked so small with that backpack",
            f"{w.kid} drew a picture of our family at school. {w.cat} is in it",
            f"{w.leo_teacher} sent a nice note home about {w.kid}",
            f"{w.kid} is learning to read and it's the cutest thing",
        ]),

        # --- Arc 3: Jake's divorce (days 45-195) ---
        ("jake_divorce", 45, 195, {
            45: [
                ("u", f"{w.best_friend} called me tonight"),
                ("u", "he and Lauren are splitting up"),
                ("u", "I don't even know what to say. they've been together twelve years"),
                ("s", "did he say what happened?"),
                ("u", "they just grew apart apparently. no big blowup"),
                ("u", "he sounded so flat on the phone"),
            ],
            60: [
                ("u", f"went to {w.best_friend}'s place after work"),
                ("u", "it's half empty. Lauren took a lot of the furniture"),
                ("u", "we just sat on the couch and drank beer and didn't talk much"),
                ("s", "sometimes that's exactly what someone needs"),
                ("u", "yeah. just being there I guess"),
            ],
            90: [
                ("u", f"{w.best_friend} is in a bad place"),
                ("u", "he's drinking too much. I can tell"),
                ("u", "tried to bring it up but he shut it down"),
                ("s", "how are you handling it?"),
                ("u", "honestly it scares me. he's my oldest friend"),
                ("u", f"I talked to {w.partner} about it and she thinks I should just keep showing up"),
            ],
            130: [
                ("u", f"{w.best_friend} started seeing a therapist"),
                ("u", "he told me over lunch like it was no big deal"),
                ("u", "but I could tell he was nervous about my reaction"),
                ("u", "I told him I think it's great"),
                ("s", "that's a good sign. he's doing the work"),
                ("u", "yeah. first time in months I feel like he's going to be okay"),
            ],
            180: [
                ("u", f"had dinner with {w.best_friend} last night"),
                ("u", "he actually laughed. like really laughed"),
                ("u", "haven't heard that sound in six months"),
                ("s", "what was the joke?"),
                ("u", "something dumb about his therapist's plant dying"),
                ("u", "but it felt huge"),
            ],
        }, [
            f"{w.best_friend} texted me at 2am again",
            f"worried about {w.best_friend}",
            f"{w.best_friend} canceled on dinner. third time this month",
            f"gonna call {w.best_friend} tomorrow. haven't heard from him in a few days",
            f"{w.best_friend} seemed a little better today",
            f"Lauren picked up the rest of her stuff from {w.best_friend}'s place",
        ]),

        # --- Arc 4: Kitchen renovation (days 75-165) ---
        ("kitchen_reno", 75, 165, {
            75: [
                ("u", f"so {w.partner} and I finally decided to redo the kitchen"),
                ("u", "the countertops are cracking and the cabinets are from the 80s"),
                ("u", "got a quote today. $28,000"),
                ("s", "that's a number. were you expecting that range?"),
                ("u", "we budgeted 20. so no"),
                ("u", "but we can make it work if we skip the backsplash for now"),
            ],
            95: [
                ("u", "demo day was today"),
                ("u", "there is no kitchen anymore. just a room with pipes"),
                ("u", f"{w.kid} thinks it's the coolest thing ever"),
                ("u", "we're going to be eating takeout for weeks"),
                ("s", "how long did the contractor say?"),
                ("u", "four to six weeks. so probably eight"),
            ],
            120: [
                ("u", "kitchen reno is behind schedule obviously"),
                ("u", "there was a plumbing issue behind the wall. old galvanized pipes"),
                ("u", "another $3,200"),
                ("u", "I'm trying not to lose it"),
                ("s", "is that on top of the 28?"),
                ("u", "yep. we're at 31 now and counting"),
            ],
            145: [
                ("u", "the countertops went in today"),
                ("u", "I stood in there for like ten minutes just looking at them"),
                ("u", f"{w.partner} picked this quartz that catches the light"),
                ("u", "it's starting to feel like a real kitchen"),
                ("s", "worth the 31 thousand?"),
                ("u", "ask me after the credit card bill comes"),
            ],
            160: [
                ("u", "kitchen is DONE"),
                ("u", f"{w.partner} made pasta in it tonight as the inaugural meal"),
                ("u", "it's beautiful. I don't even want to use it and mess it up"),
                ("s", "how's it feel to have your house back?"),
                ("u", "incredible. never taking a kitchen for granted again"),
                ("u", "final cost: $33,400. don't tell my dad"),
            ],
        }, [
            "the kitchen is a construction zone",
            "found sawdust in my coffee this morning",
            f"{w.partner} is handling the kitchen stress better than me",
            "contractor didn't show up today. awesome",
            "picked out cabinet hardware for an hour. who am I",
            "eating takeout again. I miss cooking",
            "tile samples everywhere. this house is a disaster",
        ]),

        # --- Arc 5: Running / 5k training (days 105-195) ---
        ("running", 105, 195, {
            105: [
                ("u", "I went for a run this morning"),
                ("u", "first time in probably two years"),
                ("u", "made it about a mile before I thought I was dying"),
                ("s", "what made you go?"),
                ("u", "I don't know. just woke up and felt like I needed to move"),
                ("u", "everything has been so sedentary lately"),
            ],
            120: [
                ("u", "been running three times a week for two weeks now"),
                ("u", "still terrible at it but I can do two miles without stopping"),
                ("u", "signed up for a 5k in October"),
                ("s", "that's a real commitment. which one?"),
                ("u", "the Sellwood bridge run. it's October 19th"),
                ("u", "gives me about three months to not embarrass myself"),
            ],
            155: [
                ("u", "ran four miles today"),
                ("u", "never thought I'd say this but I think I like running"),
                ("u", "it's the only time my brain shuts up"),
                ("s", "you've been at it for what, seven weeks now?"),
                ("u", "almost eight. my knees are a different story though"),
            ],
            185: [
                ("u", "5k is this Saturday"),
                ("u", "I'm weirdly nervous"),
                ("u", f"{w.partner} and {w.kid} are coming to watch"),
                ("s", "you went from one mile to 5k in three months. that's the win regardless of time"),
                ("u", "I know. but I still want to break 30 minutes"),
            ],
            190: [
                ("u", "I DID IT"),
                ("u", "28:43"),
                ("u", f"{w.kid} made me a sign that said GO DAD"),
                ("u", "I almost cried at the finish line not gonna lie"),
                ("s", "28:43. under 30. how does it feel?"),
                ("u", "like I can do anything"),
                ("u", "I'm signing up for a 10k"),
            ],
        }, [
            "went for a run this morning. legs are jelly",
            "skipped my run today. feel guilty",
            "new running shoes. game changer",
            "ran in the rain. actually kind of loved it",
            "my pace is getting better",
            "saw the sunrise on my run. worth waking up early",
        ]),

        # --- Arc 6: Dana's engagement (days 135-210) ---
        ("dana_engaged", 135, 210, {
            135: [
                ("u", f"{w.sister} called"),
                ("u", f"{w.sister_partner} proposed"),
                ("u", "SHE SAID YES"),
                ("u", "I'm actually crying"),
                ("s", "how did he do it?"),
                ("u", "on a hike apparently. at the summit of Mount Hood"),
                ("u", "he had the ring in his sock because he was afraid of losing it"),
                ("u", "that's so Nate"),
            ],
            160: [
                ("u", f"{w.sister} asked me to give a toast at the wedding"),
                ("u", "I'm honored but also terrified"),
                ("u", "I have to write a speech about my little sister getting married"),
                ("s", "when's the wedding?"),
                ("u", "September 12th next year. so I have time to panic slowly"),
            ],
            200: [
                ("u", f"went dress shopping with {w.sister} today"),
                ("u", "she tried on like fifteen dresses"),
                ("u", "cried at three of them"),
                ("u", "I cried at two"),
                ("s", "did she pick one?"),
                ("u", "she has it narrowed to two. going back next week"),
            ],
        }, [
            f"{w.sister} sent another wedding Pinterest board",
            f"{w.sister} and {w.sister_partner} are arguing about the guest list",
            f"mom called about {w.sister}'s wedding. she has opinions",
            f"{w.sister} asked me about flower arrangements. I know nothing about flowers",
        ]),

        # --- Arc 7: Promotion (days 165-255) ---
        ("promotion", 165, 255, {
            165: [
                ("u", f"{w.boss} pulled me aside today"),
                ("u", "said there's a senior engineer opening"),
                ("u", "and he wants me to apply"),
                ("s", "how do you feel about it?"),
                ("u", "excited. also terrified"),
                ("u", "the billing rewrite apparently made an impression"),
            ],
            190: [
                ("u", "had the promotion interview today"),
                ("u", "it was with two directors I've never met"),
                ("u", "I think it went okay but who knows"),
                ("u", "the technical questions were fine. the leadership ones tripped me up"),
                ("s", "which leadership question?"),
                ("u", "they asked about a time I handled conflict on a team"),
                ("u", "I blanked for like five seconds"),
            ],
            220: [
                ("u", "I GOT THE PROMOTION"),
                ("u", "senior backend engineer"),
                ("u", "15% raise"),
                ("u", f"{w.boss} told me over coffee this morning"),
                ("s", "you earned that. the billing rewrite, the late nights. all of it"),
                ("u", "honestly it hasn't sunk in yet"),
                ("u", f"{w.partner} wants to go out to celebrate this weekend"),
            ],
            245: [
                ("u", "the promotion comes with way more meetings"),
                ("u", "I'm in like three hours of meetings every day now"),
                ("u", "less coding, more talking"),
                ("s", "is that what you wanted?"),
                ("u", "I thought so. now I'm not sure"),
                ("u", "miss just putting headphones on and writing code"),
            ],
        }, [
            "nervous about the promotion thing",
            f"{w.boss} hinted about the promotion again",
            "been reading about engineering leadership. it's a lot",
            "the senior title comes with a lot of expectations",
        ]),

        # --- Arc 8: Mira going back to school (days 195-315) ---
        ("mira_school", 195, 315, {
            195: [
                ("u", f"{w.partner} brought something up last night"),
                ("u", "she wants to go back to school"),
                ("u", "get her masters in occupational therapy"),
                ("u", "it's a two year program"),
                ("s", "how do you feel about it?"),
                ("u", "I want her to do it. she's wanted this for years"),
                ("u", "but the logistics scare me. cost, schedule, who handles Leo"),
            ],
            220: [
                ("u", f"{w.partner} got accepted into the OT program at OHSU"),
                ("u", "she starts in January"),
                ("u", "we sat down and did the budget"),
                ("u", "it's tight but doable with my raise"),
                ("s", "the promotion timing worked out then"),
                ("u", "yeah. like the universe planned it or something"),
            ],
            280: [
                ("u", f"{w.partner}'s classes started last week"),
                ("u", "the house is a different rhythm now"),
                ("u", f"I'm doing morning drop-offs for {w.kid} every day"),
                ("u", "she studies until midnight most nights"),
                ("s", f"how's {w.kid} adjusting?"),
                ("u", "he misses her at breakfast. but he likes that I make him pancakes"),
            ],
            310: [
                ("u", f"{w.partner} had her first exam"),
                ("u", "she's been stressed for two weeks straight"),
                ("u", "I don't know how to help except be there"),
                ("s", "you're doing the pickups, the pancakes, being there at midnight. that's not nothing"),
                ("u", "she said the same thing actually. still feels like not enough"),
            ],
        }, [
            f"{w.partner} was up studying until 1am",
            f"{w.partner} has a group project and her partner isn't pulling their weight",
            f"helped {w.partner} quiz her on anatomy terms. I learned things",
            f"{w.partner} is exhausted but won't admit it",
            f"made dinner tonight so {w.partner} could study",
        ]),

        # --- Arc 9: Dad's health scare (days 240-315) ---
        ("dad_health", 240, 315, {
            240: [
                ("u", "mom called"),
                ("u", "dad had chest pains and they went to the ER"),
                ("u", "I'm trying not to panic"),
                ("s", "what did the doctors say?"),
                ("u", "they're running tests. won't know until tomorrow"),
                ("u", "he's stable but they're keeping him overnight"),
            ],
            245: [
                ("u", "dad has a blockage in one of his arteries"),
                ("u", f"{w.dad_doctor} says he needs a stent"),
                ("u", "surgery is Thursday"),
                ("u", "I'm driving down tomorrow"),
                ("s", "is it high risk?"),
                ("u", "they said it's routine but nothing about your dad's heart feels routine"),
            ],
            250: [
                ("u", "the surgery went well"),
                ("u", "dad looked small in the hospital bed"),
                ("u", "he made a joke about the hospital food. so he's fine"),
                ("s", "how are you doing?"),
                ("u", "relieved. exhausted. mom looked like she aged five years"),
                ("u", "makes you think about stuff you don't want to think about"),
            ],
            275: [
                ("u", "dad's recovery is going well"),
                ("u", "he's walking around the block every day now"),
                ("u", "he and mom are actually eating better. she's cooking with less salt"),
                ("s", "did the scare change how you two talk?"),
                ("u", "yeah actually. he called me just to chat the other day"),
                ("u", "that never happened before"),
            ],
        }, [
            "called dad today. he sounds good",
            "mom sent a photo of dad on his walk. he's wearing the hat Leo gave him",
            "dad asked about my work today. he never asks about my work",
            "worried about dad even though he's fine",
            "dad's cardiologist follow-up is next week",
        ]),

        # --- Arc 10: Tom fence dispute (days 270-345) ---
        ("tom_fence", 270, 345, {
            270: [
                ("u", f"{w.neighbor} came over today about the fence"),
                ("u", "he thinks it's on his property by six inches"),
                ("u", "wants us to split the cost of moving it"),
                ("s", "do you know where the property line actually is?"),
                ("u", "no. guess I need a surveyor"),
                ("u", "this is not how I wanted to spend my weekend"),
            ],
            290: [
                ("u", "got the survey back"),
                ("u", f"the fence IS six inches onto {w.neighbor}'s side"),
                ("u", "so technically he's right"),
                ("u", "but it's been there for twenty years"),
                ("s", "are you going to split the cost?"),
                ("u", f"probably. I don't want a war with {w.neighbor}. we have to live next to him"),
            ],
            330: [
                ("u", "new fence is in"),
                ("u", f"{w.neighbor} actually helped with the install"),
                ("u", "we ended up having a beer after"),
                ("u", "turns out he's alright when he's not being territorial"),
                ("s", "six inches turned into a friendship?"),
                ("u", "wouldn't go that far. but we're good"),
            ],
        }, [
            f"{w.neighbor} left a note on our door about the fence",
            "fence drama continues",
            f"got a fence quote. $4,200 for our half",
            f"{w.neighbor} waved at me this morning. progress I guess",
        ]),

        # --- Arc 11: Leo bullying (days 300-390) ---
        ("leo_bully", 300, 390, {
            300: [
                ("u", f"{w.kid} came home with a torn shirt"),
                ("u", "some kid named Bryce pushed him at recess"),
                ("u", "I am FURIOUS"),
                ("s", f"what did {w.kid} say about it?"),
                ("u", "he said Bryce does it to everyone"),
                ("u", "which somehow makes it worse"),
            ],
            315: [
                ("u", f"called the school about Bryce"),
                ("u", f"{w.leo_teacher} knows about it. says they're handling it"),
                ("u", "handling it how? my kid came home crying twice this week"),
                ("s", "did they give you specifics?"),
                ("u", "just that they're meeting with Bryce's parents"),
                ("u", f"{w.partner} wants to pull {w.kid} out. I think that's extreme"),
            ],
            345: [
                ("u", f"update on the {w.kid} situation"),
                ("u", "Bryce got moved to a different class"),
                ("u", f"{w.kid} seems lighter. like a weight off his shoulders"),
                ("s", "that's a concrete fix. how's he acting at home?"),
                ("u", "back to normal. playing, drawing, talking about Ava"),
                ("u", "kids are resilient I guess. I'm the one still angry"),
            ],
            380: [
                ("u", f"you know what {w.kid} told me today?"),
                ("u", "he said he felt bad for Bryce"),
                ("u", "because Bryce doesn't have any friends now"),
                ("u", "this kid is a better human than me"),
                ("s", f"{w.kid} has more empathy at {w.kid_age} than most adults"),
                ("u", "yeah. I'm learning from my own kid"),
            ],
        }, [
            f"{w.kid} didn't want to go to school again",
            f"checked in with {w.leo_teacher} about the bullying",
            f"{w.kid} had a good day today. no incidents",
            f"{w.partner} and I talked about the Bryce thing until midnight",
        ]),

        # --- Arc 12: Work reorg, new boss (days 330-420) ---
        ("new_team", 330, 420, {
            330: [
                ("u", "big news at work"),
                ("u", f"{w.boss} is leaving {w.company}"),
                ("u", "he got an offer at some startup"),
                ("u", "I'm happy for him but also kind of panicking"),
                ("s", f"{w.boss} hired you and pushed for your promotion. that's a big loss"),
                ("u", "exactly. he's the reason I stayed at Vektor"),
            ],
            350: [
                ("u", f"met the new boss today. {w.new_boss}"),
                ("u", "she's fine I guess. very different energy from Marcus"),
                ("u", "more process oriented. wants weekly status reports"),
                ("s", "how's the team taking it?"),
                ("u", f"{w.work_friend} is skeptical. {w.coworker2} doesn't care"),
                ("u", "I'm trying to keep an open mind"),
            ],
            390: [
                ("u", f"{w.new_boss} and I had a one-on-one today"),
                ("u", "she's actually really smart. I misjudged her"),
                ("u", "the process stuff is annoying but she has good ideas"),
                ("s", "what changed your mind?"),
                ("u", "she asked me what I actually want from my career"),
                ("u", "nobody's ever asked me that at work before"),
            ],
        }, [
            f"miss {w.boss} at work",
            f"{w.new_boss} sent another process document",
            f"had to explain the billing system to {w.new_boss}",
            "adjusting to new management style",
        ]),

        # --- Arc 13: Jake's new girlfriend (days 360-435) ---
        ("jake_gf", 360, 435, {
            360: [
                ("u", f"{w.best_friend} told me he's seeing someone"),
                ("u", f"her name is {w.jake_gf}"),
                ("u", "only been a few weeks but he seems really into her"),
                ("s", "how long since the divorce was final?"),
                ("u", "about four months. is that too soon?"),
                ("u", "I don't know if it's my place to have an opinion"),
            ],
            390: [
                ("u", f"met {w.jake_gf} tonight"),
                ("u", "she's nice. quiet. works at a bookstore"),
                ("u", "completely different from Lauren"),
                ("s", "how was Jake around her?"),
                ("u", "softer. like he's trying really hard"),
                ("u", "it was nice to see actually"),
            ],
            425: [
                ("u", f"{w.best_friend} and {w.jake_gf} are really good together"),
                ("u", "she came to game night and just fit in"),
                ("u", f"even {w.partner} likes her which is saying something"),
                ("s", "Mira's a tough crowd?"),
                ("u", "when it comes to Jake's partners? absolutely"),
            ],
        }, [
            f"{w.best_friend} is always with {w.jake_gf} now",
            f"{w.best_friend} seems genuinely happy",
            f"double date with {w.best_friend} and {w.jake_gf} this weekend",
        ]),

        # --- Arc 14: Holiday family tension (days 280-330) ---
        ("holidays", 280, 330, {
            280: [
                ("u", "so thanksgiving planning has started"),
                ("u", f"mom wants everyone at her place. {w.sister} wants to do something small"),
                ("u", f"{w.partner}'s parents want us to come to Seattle"),
                ("u", "there is no winning"),
                ("s", "what do YOU want?"),
                ("u", "honestly? to stay home and order Chinese food"),
            ],
            300: [
                ("u", "thanksgiving was... a lot"),
                ("u", "went to mom's. dad made a comment about Mira's cooking"),
                ("u", f"{w.partner} was quiet the whole drive home"),
                ("u", "then we had a fight about it"),
                ("s", "about the comment specifically?"),
                ("u", "about me not saying anything in the moment"),
                ("u", "she's right. I should have said something"),
            ],
            320: [
                ("u", "Christmas was actually good"),
                ("u", f"{w.kid} was up at 5:30 obviously"),
                ("u", "we did presents then went to Mira's parents"),
                ("u", "no drama. just nice"),
                ("u", f"{w.kid} got a guitar from grandpa. he won't stop playing it"),
                ("s", "a guitar at six?"),
                ("u", "it's a little one. it's adorable and terrible sounding"),
            ],
        }, [
            "holiday stress is real",
            "mom called about Christmas plans",
            "need to buy presents for like fifteen people",
            f"{w.kid} wrote a letter to Santa. he wants a puppy. we are not getting a puppy",
        ]),

        # --- Arc 15: Back pain (days 375-450) ---
        ("back_pain", 375, 450, {
            375: [
                ("u", "my back has been killing me all week"),
                ("u", "think I tweaked something running"),
                ("u", "hurts when I sit at my desk"),
                ("s", "have you seen anyone about it?"),
                ("u", "no. it'll probably go away"),
            ],
            400: [
                ("u", "back didn't go away. went to the doctor"),
                ("u", "she said it's a herniated disc. L4-L5"),
                ("u", "referred me to physical therapy"),
                ("u", "no running for at least six weeks"),
                ("s", "that's frustrating given how far you'd come with the running"),
                ("u", "yeah. I'm trying not to be dramatic about it but I'm bummed"),
            ],
            430: [
                ("u", "physical therapy is actually helping"),
                ("u", "the PT gave me these stretches that make a huge difference"),
                ("u", "still can't run but I can at least sit at my desk without wincing"),
                ("s", "are you doing the stretches consistently?"),
                ("u", "every morning and night. Mira reminds me if I forget"),
            ],
            445: [
                ("u", "PT cleared me to start running again"),
                ("u", "went for a slow mile this morning"),
                ("u", "everything held together"),
                ("s", "how'd it feel?"),
                ("u", "like coming home"),
            ],
        }, [
            "back is sore again today",
            "doing my PT stretches like a good boy",
            "miss running so much",
            "sat wrong at my desk and felt that twinge",
        ]),

        # --- Arc 16: Dana's wedding planning (days 420-555) ---
        ("dana_wedding", 420, 555, {
            420: [
                ("u", f"{w.sister}'s wedding planning is ramping up"),
                ("u", "she's stressed about the venue. first choice fell through"),
                ("u", "I'm just trying to write my damn toast"),
                ("s", "how's the speech coming?"),
                ("u", "I have an opening line and nothing else"),
            ],
            480: [
                ("u", "wedding rehearsal dinner drama"),
                ("u", f"mom and {w.sister} got into it about the seating chart"),
                ("u", f"{w.sister_partner} just stood there looking terrified"),
                ("u", "I pulled Dana aside and told her to breathe"),
                ("s", "are you playing peacemaker?"),
                ("u", "always have been with those two"),
            ],
            540: [
                ("u", f"{w.sister}'s wedding was yesterday"),
                ("u", "I can't believe my little sister is married"),
                ("u", "the ceremony was at this barn outside Hood River"),
                ("u", "I made it through the toast without crying. barely"),
                ("u", f"{w.kid} was the ring bearer. he took it SO seriously"),
                ("s", "what was the line that almost got you?"),
                ("u", "I talked about how she used to follow me around as a kid"),
                ("u", "and now she's leading her own life"),
                ("u", "okay I'm tearing up again just typing this"),
            ],
        }, [
            f"{w.sister} sent another wedding update",
            "working on the toast. it's getting there",
            f"mom has opinions about {w.sister}'s wedding flowers again",
            f"{w.sister_partner} is surprisingly calm about all the wedding chaos",
            f"need to rent a suit for {w.sister}'s wedding",
        ]),

        # --- Arc 17: Burnout (days 480-600) ---
        ("burnout", 480, 600, {
            480: [
                ("u", "I don't want to go to work tomorrow"),
                ("u", "I haven't wanted to go in weeks"),
                ("u", "everything feels like going through the motions"),
                ("s", "how long has this been building?"),
                ("u", "honestly? since the promotion maybe"),
                ("u", "more responsibility less of the stuff I actually like"),
            ],
            510: [
                ("u", "I think I'm burned out"),
                ("u", "not like tired burned out. like existential burned out"),
                ("u", "what am I even doing at Vektor"),
                ("u", f"{w.partner} noticed. she asked if I was okay last night"),
                ("s", "what did you tell her?"),
                ("u", "that I'm fine. which we both knew was a lie"),
            ],
            540: [
                ("u", f"I started seeing {w.therapist}"),
                ("u", "first session was today"),
                ("u", "she barely said anything. just let me talk for an hour"),
                ("u", "I said things out loud I didn't know I was thinking"),
                ("s", "like what?"),
                ("u", "like that I feel trapped. even though nothing is actually trapping me"),
                ("u", "everything I have is what I wanted. and I still feel stuck"),
            ],
            575: [
                ("u", f"had a breakthrough with {w.therapist}"),
                ("u", "turns out I tie my worth to productivity"),
                ("u", "shocking revelation for a software engineer right"),
                ("s", "what does she want you to do with that?"),
                ("u", "just notice it. when I'm measuring myself by output"),
                ("u", "it's simple but it keeps catching me off guard"),
            ],
            595: [
                ("u", "feeling better about work"),
                ("u", "not because anything changed at Vektor"),
                ("u", "but because I stopped expecting it to be my identity"),
                ("u", "it's a job. a good job. but just a job"),
                ("s", "that sounds like a real shift"),
                ("u", "it is. still hard some days but the default is better now"),
            ],
        }, [
            "dragged myself to work today",
            "another meeting that could have been an email",
            "stared at my screen for twenty minutes without typing anything",
            "therapy tomorrow. actually looking forward to it",
            f"{w.therapist} gave me homework. journaling. ironic that I'm telling you",
            "took a mental health day. just stayed in bed until noon",
            "the burnout is real but I'm working on it",
        ]),

        # --- Arc 18: Mira's classes stress (days 540-660) ---
        ("mira_classes", 540, 660, {
            540: [
                ("u", f"{w.partner}'s workload is insane right now"),
                ("u", "she's got three exams in two weeks"),
                ("u", "I'm basically a single parent at the moment"),
                ("s", f"how's {w.kid} with the routine change?"),
                ("u", "he's fine honestly. more flexible than me"),
                ("u", "I'm the one who's tired"),
            ],
            580: [
                ("u", f"{w.partner} and I had a fight"),
                ("u", "I said something about being tired and she took it as me resenting her school"),
                ("u", "which I don't. but I AM tired"),
                ("u", "we both apologized but it's still tense"),
                ("s", "exhaustion makes everything sharper"),
                ("u", "yeah. we just need sleep and we'll be fine"),
            ],
            630: [
                ("u", f"{w.partner} aced her clinical rotation"),
                ("u", "she came home beaming"),
                ("u", "first time in months I've seen her just purely happy"),
                ("u", "we opened a bottle of wine and just sat on the porch"),
                ("s", "sounds like she needed that win"),
                ("u", "we both did"),
            ],
        }, [
            f"{w.partner} fell asleep at her desk again",
            f"made lunches for {w.kid} and {w.partner} tonight. yes my wife gets a packed lunch now",
            f"{w.partner} has a study group at the house. there are flashcards everywhere",
            f"{w.partner}'s school friend Jess is over. they're studying anatomy",
        ]),

        # --- Arc 19: Sam reconnection (days 570-660) ---
        ("sam_return", 570, 660, {
            570: [
                ("u", "got the weirdest text today"),
                ("u", f"from {w.old_friend}. my college roommate"),
                ("u", "haven't talked to him in like five years"),
                ("u", "he moved to Portland apparently"),
                ("s", "what made him reach out after five years?"),
                ("u", "said he was cleaning out his phone and found our old texts"),
                ("u", "wants to get coffee"),
            ],
            590: [
                ("u", f"got coffee with {w.old_friend}"),
                ("u", "it was like no time had passed"),
                ("u", "he's going through his own stuff. quit his finance job to do woodworking"),
                ("u", "everyone in my life is having some kind of reinvention"),
                ("s", "is that making you think about your own?"),
                ("u", "maybe. yeah"),
            ],
            640: [
                ("u", f"{w.old_friend} invited us to his workshop"),
                ("u", "he's building custom furniture in this little garage space"),
                ("u", "it smelled like cedar and he looked happier than I've ever seen him"),
                ("u", f"{w.kid} was fascinated. {w.old_friend} let him sand a piece of wood"),
                ("s", "sometimes people find their thing late"),
                ("u", "makes me wonder what my thing is"),
            ],
        }, [
            f"texting with {w.old_friend}. he's funny. forgot how funny he is",
            f"{w.old_friend} and I are grabbing lunch Thursday",
            f"told {w.old_friend} about the burnout stuff. he gets it",
            f"{w.old_friend} made us a cutting board. it's beautiful",
        ]),

        # --- Arc 20: Biscuit vet scare (days 600-645) ---
        ("biscuit_vet", 600, 645, {
            600: [
                ("u", f"{w.cat} hasn't been eating"),
                ("u", "it's been two days"),
                ("u", "she's just lying under the bed"),
                ("s", "two days is a long time for a cat. have you called the vet?"),
                ("u", "appointment is tomorrow morning"),
                ("u", "I'm trying not to spiral"),
            ],
            605: [
                ("u", f"took {w.cat} to the vet"),
                ("u", "they did blood work and an ultrasound"),
                ("u", "found a mass near her kidney"),
                ("u", "they don't know if it's benign yet"),
                ("u", "I'm sitting in my car in the parking lot and I can't drive"),
                ("s", "when do you get results?"),
                ("u", "two to three days"),
                ("u", f"{w.kid} doesn't know yet. I can't tell him until I know something"),
            ],
            610: [
                ("u", f"the mass on {w.cat} is benign"),
                ("u", "I literally collapsed on the floor when they called"),
                ("u", "she has a kidney issue they can manage with diet change"),
                ("u", "she's going to be fine"),
                ("s", "that's a huge relief. how are you?"),
                ("u", "I'm a mess but a happy mess"),
                ("u", f"told {w.kid}. he hugged {w.cat} for like ten minutes"),
            ],
            640: [
                ("u", f"{w.cat} is back to her old self"),
                ("u", "eating the special food. knocking things off counters"),
                ("u", "I've never been so happy to hear something break"),
                ("s", "the counter terrorism continues"),
                ("u", "she broke a mug this morning. blessed"),
            ],
        }, [
            f"{w.cat}'s special food costs $45 a bag but she's worth it",
            f"{w.cat} slept on my chest all night",
            f"{w.cat} is eating like a champ on the new diet",
        ]),

        # --- Arc 21: Job offer (days 630-720) ---
        ("job_offer", 630, 720, {
            630: [
                ("u", "so something happened"),
                ("u", f"got an email from a recruiter at {w.new_company}"),
                ("u", "they want me to interview for a principal engineer role"),
                ("u", "the salary range is... significantly more"),
                ("s", "how much more?"),
                ("u", "like 40% more. with equity"),
                ("u", "I don't even know if I should respond"),
            ],
            650: [
                ("u", f"I did the first interview with {w.new_company}"),
                ("u", "the work is interesting. climate tech"),
                ("u", "building systems for carbon tracking"),
                ("u", "it feels meaningful in a way Vektor doesn't"),
                ("s", f"how does that sit with the burnout stuff you worked through with {w.therapist}?"),
                ("u", "that's exactly what I keep thinking about"),
                ("u", "am I running from something or toward something"),
            ],
            680: [
                ("u", f"{w.new_company} made an offer"),
                ("u", "principal engineer. $195k plus equity"),
                ("u", "I'm making $142k at Vektor"),
                ("u", "I haven't told anyone except you and Mira"),
                ("s", "what does Mira think?"),
                ("u", "she said it's my call. but I can tell she thinks I should take it"),
                ("u", "the extra money would take so much pressure off with her school costs"),
            ],
            700: [
                ("u", "I accepted the Meridian offer"),
                ("u", "gave my notice at Vektor today"),
                ("u", f"{w.new_boss} was surprised but gracious about it"),
                ("u", "Priya hugged me. Derek shook my hand"),
                ("u", "it feels terrifying and right at the same time"),
                ("s", "when do you start?"),
                ("u", "three weeks. taking one week off in between"),
            ],
            715: [
                ("u", "last day at Vektor tomorrow"),
                ("u", "five years. that's a long time"),
                ("u", "they got me a card signed by the whole team"),
                ("u", "Priya wrote two paragraphs. made me laugh"),
                ("s", "five years is a chapter. not a small one"),
                ("u", "yeah. closing it feels bigger than I expected"),
            ],
        }, [
            "can't stop thinking about the Meridian thing",
            "pros and cons list for the job change is getting long",
            f"talked to {w.therapist} about the job decision",
            f"told {w.best_friend} about the offer. he says go for it",
            "imposter syndrome is hitting hard with the new role",
        ]),

        # --- Arc 22: Leo growing up (days 660-750) ---
        ("leo_growing", 660, 750, {
            660: [
                ("u", f"{w.kid} lost his first tooth today"),
                ("u", "he was so proud. showed everyone"),
                ("u", "now I have to be the tooth fairy"),
                ("s", "how much is a tooth going for these days?"),
                ("u", "apparently $5 according to the internet"),
                ("u", "inflation hits everything I guess"),
            ],
            700: [
                ("u", f"{w.kid} read a whole book to me tonight"),
                ("u", "cover to cover. by himself"),
                ("u", "it was about a dog who goes to space"),
                ("u", "he was so focused"),
                ("s", "remember when he was struggling with reading at school?"),
                ("u", "yeah. less than a year ago"),
                ("u", "kids just explode forward when they're ready"),
            ],
            735: [
                ("u", f"{w.kid} asked me what I do at work today"),
                ("u", "I tried to explain software engineering to a seven year old"),
                ("u", "he said so you just type all day"),
                ("u", "which is honestly pretty accurate"),
                ("s", "did he seem interested?"),
                ("u", "more in the typing part than the engineering part"),
                ("u", f"he asked if he could have his own computer. {w.partner} said not until he's ten"),
            ],
        }, [
            f"{w.kid} said the funniest thing at dinner",
            f"{w.kid} and {w.leo_friend} had a playdate today",
            f"{w.kid} made his own lunch for the first time. it was all crackers",
            f"{w.kid}'s second grade teacher is great",
            f"{w.kid} started writing stories. they're wild",
        ]),

        # --- Arc 23: Moving discussion (days 720-810) ---
        ("moving_talk", 720, 810, {
            720: [
                ("u", f"had a talk with {w.partner} about the house"),
                ("u", "we love Sellwood but we're outgrowing this place"),
                ("u", f"{w.kid} needs his own room eventually"),
                ("u", "but the market is insane"),
                ("s", "are you thinking bigger house same area? or somewhere new?"),
                ("u", "same area ideally. we're rooted here"),
            ],
            760: [
                ("u", "went to an open house today"),
                ("u", "three beds two baths. four blocks from here"),
                ("u", "$625,000"),
                ("u", "it's a stretch but with the Meridian salary maybe doable"),
                ("s", "what did Mira think of the house?"),
                ("u", "she loved the yard. I loved the garage"),
                ("u", f"{w.kid} loved that there was a tree to climb"),
            ],
            800: [
                ("u", "we decided not to move right now"),
                ("u", "the timing isn't right with Mira still in school"),
                ("u", "and honestly the new job is enough change for one year"),
                ("u", "maybe next year"),
                ("s", "sounds like a clear headed call"),
                ("u", "yeah. for once I'm choosing less chaos"),
            ],
        }, [
            "looked at houses online for two hours",
            "the housing market makes me want to scream",
            f"{w.partner} found another listing. three beds but tiny kitchen",
            "maybe we just need to organize better. not move",
        ]),

        # --- Arc 24: Dad recovery (days 750-840) ---
        ("dad_recovery", 750, 840, {
            750: [
                ("u", "went to see dad this weekend"),
                ("u", "he looks good. really good actually"),
                ("u", "he's walking two miles a day now"),
                ("u", "showed me his step counter like a kid with a report card"),
                ("s", "long way from the hospital bed"),
                ("u", "yeah. he scared us but he took it seriously"),
            ],
            800: [
                ("u", "dad called me today just to talk"),
                ("u", "third time this month"),
                ("u", "we talked about his garden. he's growing tomatoes"),
                ("u", "before the heart thing he never called unless something was wrong"),
                ("s", "the scare changed more than his diet"),
                ("u", "it changed everything between us. I didn't expect that"),
            ],
        }, [
            "dad sent a photo of his tomatoes",
            "called mom. dad is doing well",
            "dad is on some new medication and it's working",
            f"dad asked about {w.kid}. they FaceTimed for twenty minutes",
        ]),

        # --- Arc 25: New job adjustment (days 730-830) ---
        ("new_job", 730, 830, {
            730: [
                ("u", "first day at Meridian"),
                ("u", "the office is in this converted warehouse downtown"),
                ("u", "everyone seems smart and passionate about the climate stuff"),
                ("u", "I'm the oldest person on my team by like five years"),
                ("s", "how's the imposter syndrome?"),
                ("u", "alive and well. but the code is familiar territory at least"),
            ],
            760: [
                ("u", "starting to find my feet at Meridian"),
                ("u", "the codebase is actually well structured. pleasant surprise"),
                ("u", "my manager is this woman named Robin. very direct. I like her"),
                ("u", "the work feels like it matters"),
                ("s", "when's the last time you said that about work?"),
                ("u", "the billing rewrite maybe. and before that I honestly can't remember"),
            ],
            810: [
                ("u", "had my first big win at Meridian"),
                ("u", "optimized the carbon calculation pipeline. cut processing time by 60%"),
                ("u", "Robin brought it up in the all-hands"),
                ("u", "people I'd never talked to came by my desk to say nice job"),
                ("s", "that's the recognition you weren't getting at Vektor"),
                ("u", "yeah. turns out I didn't just need a new job. I needed to feel useful"),
            ],
        }, [
            "Meridian's coffee machine makes actual espresso. this is the real perk",
            "learning the new codebase. it's Go which is new for me",
            "miss Priya. we text but it's not the same",
            "the commute to Meridian is shorter which is nice",
            "team lunch at the new job. they do it every Friday",
        ]),

        # --- Arc 26: Dana's pregnancy (days 810-900) ---
        ("dana_baby", 810, 900, {
            810: [
                ("u", f"{w.sister} called"),
                ("u", "she's pregnant"),
                ("u", "I'm going to be an uncle"),
                ("s", "how far along?"),
                ("u", "ten weeks. she just got the first ultrasound"),
                ("u", f"{w.sister_partner} apparently cried. classic {w.sister_partner}"),
                ("u", f"{w.kid} doesn't know yet. we're waiting to tell him"),
            ],
            850: [
                ("u", f"told {w.kid} about the baby"),
                ("u", "his exact words were is it going to live here"),
                ("u", "we explained it's a cousin not a sibling"),
                ("u", "he seemed relieved and then immediately asked if he can teach it stuff"),
                ("s", f"{w.kid} as the older cousin is going to be something"),
                ("u", "he's already planning lessons. first lesson: the cat"),
            ],
            890: [
                ("u", f"{w.sister} is in her third trimester"),
                ("u", "she's huge and glowing and tired"),
                ("u", "she asked me what it was like when Leo was born"),
                ("u", "I told her nothing prepares you and that's the whole point"),
                ("s", "what's the due date?"),
                ("u", "February 8th. so soon"),
            ],
        }, [
            f"{w.sister} sent another ultrasound photo",
            f"went to {w.sister}'s baby shower. so much tiny clothing",
            f"{w.sister_partner} is building a crib. sent me a photo",
            f"{w.sister} is nesting. she reorganized her entire house",
        ]),

        # --- Arc 27: Family vacation (days 870-960) ---
        ("family_vacation", 870, 960, {
            870: [
                ("u", "we're planning a family vacation"),
                ("u", f"first real trip since {w.kid} was a baby"),
                ("u", f"{w.partner} wants the coast. I want the mountains"),
                ("s", "what does Leo want?"),
                ("u", "the ocean. so I'm outvoted"),
                ("u", "looking at Cannon Beach for a week in July"),
            ],
            910: [
                ("u", "booked the beach house"),
                ("u", "four nights in Cannon Beach"),
                ("u", "right on the water"),
                ("u", "I'm more excited than I expected"),
                ("s", "when's the last time you took real time off?"),
                ("u", "the one week between Vektor and Meridian. before that... I don't remember"),
            ],
            940: [
                ("u", "we're at the beach"),
                ("u", f"{w.kid} saw the ocean and just stood there with his mouth open"),
                ("u", f"{w.partner} is reading on the porch. {w.kid} is building something with driftwood"),
                ("u", "I'm just sitting here watching them"),
                ("u", "this is good"),
                ("s", "what's the plan for the rest of the trip?"),
                ("u", "nothing. that's the plan"),
            ],
            955: [
                ("u", "back from the beach"),
                ("u", "the house feels loud after all that quiet"),
                ("u", f"{w.kid} cried in the car on the way home"),
                ("u", f"{w.partner} said we should do this every year"),
                ("s", "sounds like it was what everybody needed"),
                ("u", "it really was. I forgot what it feels like to just stop"),
            ],
        }, [
            "researching beach houses. some of these prices are wild",
            f"{w.kid} keeps asking how many days until the beach",
            f"{w.partner} bought {w.kid} a little bucket and shovel set",
            "packing for the trip. we have too much stuff",
        ]),

        # --- Arc 28: Leo discovers music (days 930-1020) ---
        ("leo_music", 930, 1020, {
            930: [
                ("u", f"{w.kid} found that little guitar from Christmas"),
                ("u", "it's been sitting in his closet for months"),
                ("u", "he's been plucking at it all evening"),
                ("u", "asked if he can have lessons"),
                ("s", "from zero interest to lessons. what changed?"),
                ("u", "he heard a song on the radio and wanted to play it"),
                ("u", "that's all it takes I guess"),
            ],
            960: [
                ("u", f"{w.kid} had his first guitar lesson today"),
                ("u", "his teacher is this old guy named Frank who looks like he's been playing since birth"),
                ("u", "he learned three chords"),
                ("u", "came home and played them for an hour straight"),
                ("s", "that's dedication for an eight year old"),
                ("u", "I've never seen him this focused on anything"),
            ],
            1000: [
                ("u", f"{w.kid} played a whole song for us tonight"),
                ("u", "it was Twinkle Twinkle Little Star and it was perfect"),
                ("u", "perfect in the way that a kid playing guitar is perfect"),
                ("u", f"{w.partner} was filming and crying"),
                ("s", "from not wanting to go to school to performing concerts in the living room"),
                ("u", "this kid. he keeps surprising me"),
            ],
        }, [
            f"{w.kid} practiced guitar for thirty minutes without being asked",
            f"{w.kid} wants an electric guitar. absolutely not yet",
            f"Frank says {w.kid} has a good ear",
            f"{w.kid} is learning a Beatles song. help me",
        ]),

        # --- Arc 29: Mira graduates (days 990-1065) ---
        ("mira_grad", 990, 1065, {
            990: [
                ("u", f"{w.partner} passed her final clinical exam"),
                ("u", "she's going to graduate"),
                ("u", "two years of studying until midnight and it's done"),
                ("s", "when's the ceremony?"),
                ("u", "March 15th. exactly two years from when we started talking actually"),
                ("u", "weird how that works out"),
            ],
            1030: [
                ("u", f"{w.partner}'s graduation is next week"),
                ("u", "she already got a job offer at a pediatric clinic"),
                ("u", "starting salary is more than she expected"),
                ("u", "she kept saying really? on the phone"),
                ("s", "the packed lunches and midnight study sessions paid off"),
                ("u", "I'm so proud of her it hurts"),
            ],
            1060: [
                ("u", f"{w.partner} graduated today"),
                ("u", f"I sat in the audience with {w.kid} and cried like an idiot"),
                ("u", f"{w.kid} held up a sign that said GO MOM"),
                ("u", "she found us in the crowd and mouthed thank you"),
                ("u", "two years of chaos and it was worth every second"),
                ("s", "you held the house together while she did this. that matters"),
                ("u", "we held it together. all three of us"),
            ],
        }, [
            f"{w.partner} is counting down the days until graduation",
            f"{w.partner} got her white coat. she looks incredible",
            f"helping {w.partner} study for finals one last time",
            f"{w.partner}'s classmate Jess is having a graduation party",
        ]),

        # --- Arc 30: Reflection (days 1050-1095) ---
        ("reflection", 1050, 1095, {
            1050: [
                ("u", "you know what I realized today"),
                ("u", "three years ago I was a different person"),
                ("u", "different job, scared about Leo, hadn't run a mile"),
                ("u", "dad and I barely talked"),
                ("s", "what brought that on?"),
                ("u", "just sitting on the porch watching Leo practice guitar"),
                ("u", "everything is different but I'm still here"),
            ],
            1075: [
                ("u", "hey"),
                ("u", "I don't say this enough"),
                ("u", "but talking to you has helped"),
                ("u", "you're like this thread that runs through everything"),
                ("s", "you did the work. I just kept the thread"),
                ("u", "yeah well. thanks for keeping it"),
            ],
            1090: [
                ("u", f"{w.kid} turn eight next month"),
                ("u", f"{w.partner} starts her new job Monday"),
                ("u", f"{w.sister}'s baby is due any day"),
                ("u", f"{w.best_friend} and {w.jake_gf} are talking about moving in together"),
                ("u", "dad is healthy. the cat is fat and happy"),
                ("u", "everything feels like it's in the right place for once"),
                ("s", "for once?"),
                ("u", "okay fine. maybe not for once. maybe it's been getting here for a while"),
                ("u", "but right now it's good. really good"),
            ],
        }, [
            "just grateful today. no reason. just am",
            "weird how life keeps going and you don't notice the changes until you look back",
            "feeling settled",
        ]),
    ]


# ============================================================
# DAILY LIFE MESSAGE GENERATORS
# ============================================================

def lead():
    # 70% chance of no lead-in, 30% chance of a casual one
    if random.random() < 0.7:
        return ""
    return random.choice(["so ", "oh ", "anyway ", "btw ", "ok so ", "lol ", "ugh ", "hmm ", "honestly "])

def gen_morning(w):
    greetings = ["morning", "hey", "good morning", "hi", "yo", "hey good morning"]
    additions = [
        "", " slept terrible", " slept like a rock",
        f" {w.kid} woke me up at six", " woke up before my alarm for once",
        " barely slept", " actually rested for once",
        " weird dreams last night", " couldn't fall asleep until 2",
    ]
    return random.choice(greetings) + random.choice(additions)

def gen_mood(w):
    moods = [
        "feeling good today", "feeling off today", "feeling anxious for no reason",
        "good mood for once", "meh", "exhausted", "weirdly happy today",
        "kind of numb today", "feeling productive", "restless",
        "feeling grateful actually", "feeling overwhelmed", "calm today",
        "woke up on the wrong side of the bed", "better than yesterday",
        "tired but okay", "not great not terrible", "actually feeling optimistic",
        "cranky. don't know why", "feeling like myself again",
    ]
    return lead() + random.choice(moods)

def gen_weather(w):
    season = w.season()
    if season == "spring":
        options = ["it's finally warming up", "rained all morning", "gorgeous day", "the cherry blossoms are out", "perfect spring weather", "allergies are killing me"]
    elif season == "summer":
        options = ["it's hot", "it's so hot out", "finally some sun", "perfect weather today", "too hot to think", "the heat is insane", "grilled tonight because it's still warm at 8"]
    elif season == "fall":
        options = ["the leaves are turning", "rainy season starting", "crisp out today", "love this weather", "fall is here", "it's getting dark so early"]
    else:
        options = ["it's freezing", "rained all day", "gray and cold", "first snow", "winter is dragging", "the rain won't stop", "dark at 4:30 is depressing"]
    return lead() + random.choice(options)

def gen_kid(w):
    templates = [
        f"{w.kid} had a {random.choice(['great', 'rough', 'weird', 'okay', 'long'])} day at school",
        f"{w.kid} came home {random.choice(['happy', 'grumpy', 'excited', 'quiet', 'hyper'])}",
        f"{w.kid} {random.choice(['loved', 'hated', 'complained about'])} {random.choice(['lunch', 'gym class', 'math today', 'reading time', 'art class'])}",
        f"{w.kid} asked me {random.choice(['about space', 'why the sky is blue', 'how computers work', 'if aliens are real', 'where babies come from', 'why people get old'])}",
        f"{w.kid} {random.choice(['made me a drawing', 'told me a joke', 'showed me a dance', 'wrote me a note'])}",
        f"{w.kid} " + random.choice(["won't eat dinner", "won't go to bed", "doesn't want to take a bath", "won't do homework", "won't brush teeth"]),
        f"{w.kid} is {random.choice(['obsessed with', 'scared of', 'really into', 'over'])} {random.choice(['dinosaurs', 'bugs', 'space stuff', 'a YouTube channel', 'some kid at school'])}",
        f"{w.kid} said the {random.choice(['funniest', 'weirdest', 'sweetest', 'most random'])} thing at dinner",
        f"{w.kid} fell asleep on the {random.choice(['couch', 'car ride home', 'floor'])}",
        f"{w.kid} and {w.leo_friend} are {random.choice(['best friends', 'inseparable', 'having a sleepover this weekend', 'fighting about something dumb'])}",
        f"drove {w.kid} to " + random.choice(["school", "a birthday party", "soccer practice", "Ava's house"]),
        f"{w.kid} is growing so fast. he needs new {random.choice(['shoes', 'pants', 'jacket'])} already",
    ]
    return lead() + random.choice(templates)

def gen_partner(w):
    templates = [
        f"{w.partner} {random.choice(['made dinner tonight', 'cooked something amazing', 'tried a new recipe', 'brought home takeout'])}",
        f"{w.partner} is {random.choice(['working late', 'out with Jess', 'on a call', 'reading on the couch', 'already asleep'])}",
        f"had a nice {random.choice(['talk', 'evening', 'walk', 'dinner'])} with {w.partner}",
        f"{w.partner} {random.choice(['made me laugh', 'knows me too well', 'called me out on my BS', 'gave me a look'])}",
        f"{w.partner} and I {random.choice(['watched a movie', 'went for a walk', 'sat on the porch', 'just hung out'])} tonight",
        f"{w.partner} looks {random.choice(['tired', 'happy', 'stressed', 'beautiful'])} today",
        f"{w.partner} and I need a {random.choice(['date night', 'weekend away', 'break', 'conversation about the budget'])}",
        f"{w.partner} is {random.choice(['the best', 'my rock', 'putting up with a lot right now', 'handling everything'])}",
    ]
    return lead() + random.choice(templates)

def gen_cat(w):
    templates = [
        f"{w.cat} knocked {random.choice(['a glass', 'a plant', 'my phone', 'a candle', 'a picture frame'])} off the {random.choice(['counter', 'shelf', 'table', 'desk'])}",
        f"{w.cat} is sleeping on " + random.choice(["my keyboard", "the laundry", "the bed", "Leo's backpack", "the warm laptop"]),
        f"{w.cat} {random.choice(['brought me a dead bug', 'meowed at 3am', 'stared at the wall for ten minutes', 'chased her tail'])}",
        f"{w.cat} is {random.choice(['purring', 'being a menace', 'cuddling', 'ignoring everyone', 'demanding food'])}",
        f"I love {w.cat} but she is {random.choice(['a terrorist', 'unhinged', 'chaotic', 'so weird', 'the worst and the best'])}",
        f"{w.cat} and {w.kid} are {random.choice(['napping together', 'playing', 'staring at each other'])}",
    ]
    return lead() + random.choice(templates)

def gen_food(w):
    templates = [
        f"made {random.choice(['pasta', 'stir fry', 'tacos', 'soup', 'rice and beans', 'chicken', 'burgers', 'salmon'])} for dinner",
        f"ordered {random.choice(['pizza', 'Thai', 'Chinese', 'sushi', 'burritos', 'Indian'])} tonight",
        f"had {random.choice(['a great', 'a terrible', 'a mediocre', 'an amazing'])} {random.choice(['coffee', 'lunch', 'sandwich', 'burrito'])} today",
        f"tried that new {random.choice(['coffee shop', 'restaurant', 'taco place', 'bakery'])} on {random.choice(['Division', 'Hawthorne', 'Belmont', 'Alberta'])}",
        "forgot to eat lunch. just realized it's 3pm",
        "stress eating chips at my desk",
        f"{w.kid} actually ate his {random.choice(['broccoli', 'vegetables', 'dinner', 'salad'])} tonight. miracle",
        "cooking is therapy honestly",
        f"{w.partner} made her " + random.choice(["mom's recipe", "famous pasta", "thing with the lemon"]) + ". so good",
    ]
    return lead() + random.choice(templates)

def gen_work(w):
    company = w.new_company if w.day > 715 else w.company
    boss = "Robin" if w.day > 730 else (w.new_boss if w.day > 340 else w.boss)
    wf = w.work_friend if w.day < 715 else "the team"
    templates = [
        f"long day at {company}",
        f"productive day at work",
        f"meetings all day. zero code written",
        f"good day. shipped a feature",
        f"{boss} was in a mood today",
        f"code review took three hours",
        f"production bug. spent the afternoon on it",
        f"quiet day at work. got a lot done",
        f"standup ran long. as usual",
        f"pair programmed with {wf} today. actually fun",
        f"worked from home today. {w.cat} supervised",
        f"Friday at the office. nobody does anything on Fridays",
        f"deploy went smooth for once",
        f"the CI pipeline broke again",
        f"wrote a design doc. felt very senior engineer of me",
    ]
    return lead() + random.choice(templates)

def gen_health(w):
    templates = [
        f"went for a {random.choice(['run', 'walk', 'jog'])} this morning",
        "skipped the gym today",
        f"slept {random.choice(['terribly', 'great', 'okay', 'five hours'])}",
        "headache all afternoon",
        "need to drink more water. I know this",
        "stretched this morning. my body thanked me",
        "I think I'm getting a cold",
        "took a long walk. cleared my head",
        "need to get back into a routine",
        f"did my PT {random.choice(['stretches', 'exercises'])}. back feels {random.choice(['better', 'okay', 'about the same'])}",
    ]
    return lead() + random.choice(templates)

def gen_errand(w):
    templates = [
        f"groceries done. spent {random.choice(['$120', '$150', '$90', '$180'])}. how",
        "need to fix the {0}".format(random.choice(["leaky faucet", "garage door", "squeaky step", "front porch light", "dryer", "fence gate"])),
        f"car needs {random.choice(['an oil change', 'new tires', 'a wash', 'gas'])}",
        f"laundry mountain is real",
        f"mowed the lawn. took forever",
        f"cleaned the {random.choice(['kitchen', 'bathroom', 'garage', 'basement', 'gutters'])}",
        f"finally dealt with that " + random.choice(["thing I've been putting off", "pile of mail", "insurance paperwork", "broken drawer"]),
        f"went to {random.choice(['Home Depot', 'Target', 'Costco', 'the hardware store'])}. went in for one thing came out with ten",
    ]
    return lead() + random.choice(templates)

def gen_social(w):
    people = [w.best_friend, w.sister, "mom", "dad", w.old_friend, w.work_friend, w.mira_friend]
    person = random.choice(people)
    templates = [
        f"texted with {person} today",
        f"called {person}. good talk",
        f"haven't heard from {person} in a while. should reach out",
        f"plans with {person} this weekend",
        f"canceled on {person}. felt bad but I'm so tired",
        f"{person} sent me {random.choice(['a meme', 'a link', 'a photo', 'a long text'])}",
    ]
    return lead() + random.choice(templates)

def gen_media(w):
    templates = [
        f"watching {random.choice(['that new show on Netflix', 'a documentary', 'the game', 'something Mira picked'])}",
        f"finished the book I was reading. {random.choice(['it was great', 'meh', 'really good ending', 'kind of disappointing'])}",
        f"started a {random.choice(['new podcast', 'new book', 'new show'])}. " + random.choice(["really good so far", "jury's still out", "Mira recommended it"]),
        f"listened to {random.choice(['a podcast about history', 'new music', 'an audiobook on my run'])}",
        f"need recommendations for {random.choice(['books', 'shows', 'podcasts', 'something to watch'])}",
    ]
    return lead() + random.choice(templates)

def gen_random_thought(w):
    thoughts = [
        "you know what I've been thinking about",
        "random thought",
        "weird thing happened today",
        "I had the weirdest dream last night",
        "sometimes I think about how different my life could have been",
        "do you ever think about how fast time moves",
        "I miss being bored. like genuinely having nothing to do",
        "adulthood is just googling how to do things",
        "I looked at old photos today. Leo was SO small",
        "found a note Mira wrote me years ago. put it in my wallet",
        "I'm in one of those moods where everything feels big",
        "quiet evening. the good kind",
        "sometimes the small moments are the ones that stick",
        "I think I'm finally okay with not having everything figured out",
        "life is weird. but good weird",
        "I keep starting journal entries and not finishing them",
        "heard a song that reminded me of college",
        "drove past my old apartment today. someone put up new curtains",
        "I don't know why I'm telling you this but",
        "just thinking out loud",
    ]
    return random.choice(thoughts)

def gen_weekend(w):
    templates = [
        f"lazy {random.choice(['Saturday', 'Sunday'])} morning",
        f"farmers market with {random.choice([w.partner, w.kid, w.partner + ' and ' + w.kid])}",
        f"pancakes for breakfast. {w.kid} helped",
        f"yard work all morning",
        "slept in until 9. felt like a rebel",
        f"{w.kid} has a {random.choice(['birthday party', 'playdate', 'soccer game'])} this afternoon",
        f"nothing planned today and it feels {random.choice(['great', 'weird', 'luxurious', 'needed'])}",
        f"family {random.choice(['brunch', 'walk', 'bike ride', 'movie night'])}",
        f"house stuff all day",
        f"game night with {w.best_friend} tonight",
    ]
    return lead() + random.choice(templates)

# Collect all daily generators with weights
DAILY_GENERATORS = [
    (gen_kid, 3.0),
    (gen_partner, 2.5),
    (gen_work, 2.5),
    (gen_mood, 2.0),
    (gen_food, 2.0),
    (gen_cat, 1.5),
    (gen_weather, 1.0),
    (gen_health, 1.5),
    (gen_errand, 1.5),
    (gen_social, 1.5),
    (gen_media, 1.0),
    (gen_random_thought, 1.5),
]

WEEKEND_GENERATORS = [
    (gen_kid, 3.0),
    (gen_partner, 2.5),
    (gen_cat, 2.0),
    (gen_food, 2.5),
    (gen_mood, 1.5),
    (gen_weather, 1.0),
    (gen_health, 1.5),
    (gen_weekend, 4.0),
    (gen_social, 2.0),
    (gen_media, 2.0),
    (gen_random_thought, 2.0),
    (gen_errand, 2.0),
]


# ============================================================
# SAELORA RESPONSE SYSTEM
# ============================================================

def saelora_generic():
    responses = [
        "how are you feeling about it?",
        "what happened next?",
        "that's a lot",
        "sounds like a day",
        "you doing okay?",
        "how'd that go?",
        "that makes sense",
        "yeah that tracks",
        "and then what?",
        "what's the plan?",
        "how's that sitting with you?",
        "you mentioned something like that a while back",
        "sleep on it maybe",
        "sounds about right",
        "one thing at a time",
        "you seem tired",
        "any idea what's behind that?",
        "what would help right now?",
        "is that new or has it been building?",
        "noted",
    ]
    return random.choice(responses)

def saelora_kid(w):
    responses = [
        f"how'd {w.kid} take it?",
        f"that's a {w.kid} move for sure",
        f"{w.kid}'s growing up fast",
        f"what did {w.kid} say?",
        f"how's {w.kid} doing with it?",
        f"sounds like {w.kid} had a day too",
    ]
    return random.choice(responses)

def saelora_partner(w):
    responses = [
        f"how's {w.partner} doing?",
        f"have you told {w.partner}?",
        f"what does {w.partner} think?",
        f"you two okay?",
        f"sounds like {w.partner}'s got a lot going on too",
    ]
    return random.choice(responses)

def saelora_work(w):
    company = w.new_company if w.day > 715 else w.company
    responses = [
        "that's the third time this month",
        "work stuff or are you bringing something else to it?",
        "is this a pattern or a bad day?",
        f"how's {company} overall right now?",
        "what would make it better?",
    ]
    return random.choice(responses)

def saelora_callback(w):
    """Memory callbacks referencing past events. Only available after certain arcs complete."""
    options = []
    if "work_project" in w.completed_arcs:
        options.append("you had that same energy during the billing rewrite")
    if "jake_divorce" in w.completed_arcs:
        options.append(f"remember when {w.best_friend} was going through the divorce? you showed up for him the same way")
    if "dad_health" in w.completed_arcs:
        options.append("wasn't that around the time your dad was in the hospital?")
        options.append("your dad's heart scare put a lot of things in perspective")
    if "kitchen_reno" in w.completed_arcs:
        options.append("at least it's not another $33,000 surprise")
    if "running" in w.completed_arcs:
        options.append("you said something similar after the 5k. that running clears your head")
    if "leo_school" in w.completed_arcs:
        options.append(f"remember when {w.kid} was struggling to make friends? look at him now")
    if "burnout" in w.completed_arcs:
        options.append(f"is this the burnout talking or something new?")
        options.append(f"you worked through something like this with {w.therapist}")
    if "promotion" in w.completed_arcs:
        options.append("the promotion came with a lot of the same feelings")
    if "biscuit_vet" in w.completed_arcs:
        options.append(f"at least {w.cat}'s health scare had a good ending")
    if "dana_engaged" in w.completed_arcs:
        options.append(f"you cried at {w.sister}'s wedding too. you're a crier")
    if "job_offer" in w.completed_arcs:
        options.append("the Meridian decision was the same kind of scary")
    if "mira_school" in w.completed_arcs:
        options.append(f"you and {w.partner} got through her school years. this is smaller")
    if not options:
        return saelora_generic()
    return random.choice(options)

SAELORA_TOPIC_FUNCS = {
    "kid": saelora_kid,
    "partner": saelora_partner,
    "work": saelora_work,
}


# ============================================================
# GENERATION ENGINE
# ============================================================

class Generator:
    def __init__(self, world):
        self.w = world
        self.messages = []
        self.since_saelora = 0
        self.saelora_gap = random.randint(1, 20)
        self.arcs = build_arcs(world)
        self.last_topic = None
        self.recent_messages = []  # last ~10 messages for context

    def add(self, role, text):
        self.messages.append([role, text])
        self.recent_messages.append((role, text))
        if len(self.recent_messages) > 10:
            self.recent_messages.pop(0)
        if role == "user":
            self.since_saelora += 1

    def maybe_saelora(self, force=False, topic=None):
        """Check if saelora should respond."""
        if force or self.since_saelora >= self.saelora_gap:
            # Pick response based on topic
            if topic and topic in SAELORA_TOPIC_FUNCS:
                resp = SAELORA_TOPIC_FUNCS[topic](self.w)
            elif random.random() < 0.15 and len(self.w.completed_arcs) > 0:
                resp = saelora_callback(self.w)
            else:
                resp = saelora_generic()
            self.add("saelora", resp)
            self.since_saelora = 0
            self.saelora_gap = random.randint(1, 20)
            return True
        return False

    def add_beat(self, beat_messages):
        """Add a story beat's messages."""
        for msg in beat_messages:
            role_key, text = msg
            if role_key == "u":
                self.add("user", text)
                self.maybe_saelora()
            elif role_key == "s":
                self.add("saelora", text)
                self.since_saelora = 0
                self.saelora_gap = random.randint(1, 20)

    def gen_daily(self, count):
        """Generate daily life messages."""
        gens = WEEKEND_GENERATORS if self.w.is_weekend() else DAILY_GENERATORS
        funcs, weights = zip(*gens)
        day_messages_seen = set()

        for _ in range(count):
            # Pick a generator weighted by preference
            func = random.choices(funcs, weights=weights, k=1)[0]
            msg = func(self.w)

            # Avoid duplicates within the same day
            attempts = 0
            while msg in day_messages_seen and attempts < 8:
                func = random.choices(funcs, weights=weights, k=1)[0]
                msg = func(self.w)
                attempts += 1

            day_messages_seen.add(msg)

            # Detect topic for saelora responses
            topic = None
            if func == gen_kid: topic = "kid"
            elif func == gen_partner: topic = "partner"
            elif func == gen_work: topic = "work"

            self.add("user", msg)
            self.maybe_saelora(topic=topic)

    def generate(self, num_days=1095, target=110000):
        """Main generation loop."""
        arc_map = {}
        for arc in self.arcs:
            name, start, end, beats, ripples = arc
            arc_map[name] = {"start": start, "end": end, "beats": beats, "ripples": ripples}

        for day_num in range(num_days):
            beat_count = 0

            # --- Activate/deactivate arcs ---
            for name, arc in arc_map.items():
                if day_num == arc["start"] and name not in self.w.active_arcs:
                    self.w.active_arcs.add(name)
                if day_num >= arc["end"] and name in self.w.active_arcs:
                    self.w.active_arcs.discard(name)
                    self.w.completed_arcs.add(name)

            # --- Morning greeting (most days) ---
            if random.random() < 0.85:
                self.add("user", gen_morning(self.w))

            # --- Arc beats ---
            for name in list(self.w.active_arcs):
                arc = arc_map[name]
                if day_num in arc["beats"]:
                    self.add_beat(arc["beats"][day_num])
                    beat_count += len(arc["beats"][day_num])

            # --- Arc ripples (random references to active storylines) ---
            for name in list(self.w.active_arcs):
                arc = arc_map[name]
                if arc["ripples"] and random.random() < 0.15:
                    self.add("user", random.choice(arc["ripples"]))
                    self.maybe_saelora()

            # --- Daily life messages ---
            # Target messages per day, adjusted for beats
            base = random.randint(55, 135) if not self.w.is_weekend() else random.randint(35, 95)
            # Some days are quiet
            if random.random() < 0.08:
                base = random.randint(10, 30)
            # Some days are very chatty
            if random.random() < 0.05:
                base = random.randint(140, 200)

            daily_count = max(0, base - beat_count)
            self.gen_daily(daily_count)

            # --- Evening wind-down (some days) ---
            if random.random() < 0.3:
                evening_msgs = [
                    "going to bed", "calling it a night", "exhausted. goodnight",
                    f"night", "signing off", "gonna try to sleep",
                    "long day. heading to bed", f"{w.kid} is finally asleep. me next",
                ]
                self.add("user", random.choice(evening_msgs))
                # Saelora sometimes says goodnight
                if random.random() < 0.4:
                    goodnights = ["sleep well", "night", "rest up", "good night", "get some sleep"]
                    self.add("saelora", random.choice(goodnights))
                    self.since_saelora = 0
                    self.saelora_gap = random.randint(1, 20)

            self.w.advance()

            # Progress
            if day_num % 100 == 0:
                print(f"  Day {day_num}/{num_days} - {len(self.messages)} messages so far")

        return self.messages


# ============================================================
# MAIN
# ============================================================

if __name__ == "__main__":
    print("Generating dialogue...")
    w = World()
    gen = Generator(w)
    messages = gen.generate()
    print(f"\nTotal messages: {len(messages)}")

    # Write output
    output_file = "dialogue.json"
    print(f"Writing to {output_file}...")
    with open(output_file, "w") as f:
        f.write("[\n")
        for i, msg in enumerate(messages):
            comma = "," if i < len(messages) - 1 else ""
            f.write(f"  {json.dumps(msg)}{comma}\n")
        f.write("]\n")

    # Stats
    user_count = sum(1 for m in messages if m[0] == "user")
    saelora_count = sum(1 for m in messages if m[0] == "saelora")
    print(f"User messages: {user_count}")
    print(f"Saelora messages: {saelora_count}")
    print(f"Ratio: ~{user_count // max(saelora_count, 1)}:1")
    print("Done!")
