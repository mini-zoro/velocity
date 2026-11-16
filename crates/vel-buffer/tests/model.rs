use proptest::prelude::*;
use vel_buffer::{
    Bias, Buffer, Change, ChangeSet, Edit, EditKind, HistoryLimits, Region, Selection, ops,
};

fn naive_point(p: usize, changes: &[(usize, usize, usize)], right: bool) -> usize {
    let mut old = 0;
    let mut new = 0;
    for &(a, b, n) in changes {
        if p < a {
            return new + p - old;
        }
        new += a - old;
        if p == a {
            return new + if right { n } else { 0 };
        }
        if p < b {
            return new;
        }
        new += n;
        old = b;
    }
    new + p - old
}
fn boundary(s: &str, n: usize) -> usize {
    s.char_indices()
        .nth(n % (s.chars().count() + 1))
        .map_or(s.len(), |(at, _)| at)
}
fn unicode() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just('a'),
            Just(' '),
            Just('\n'),
            Just('\r'),
            Just('中'),
            Just('🦀'),
            Just('\u{301}'),
            Just('\u{200d}')
        ],
        0..30,
    )
    .prop_map(|c| c.into_iter().collect())
}
proptest! {
    #[test]
    fn transaction_and_full_history_match_string(initial in unicode(),commands in prop::collection::vec((any::<usize>(),any::<usize>(),unicode()),1..45)){
        let mut b=Buffer::new(initial.as_str());let v=b.new_view();let mut model=initial;let mut steps=vec![];
        for (x,y,value) in commands{
            let a=boundary(&model,x);let z=boundary(&model,y);let range=a.min(z)..a.max(z);
            let changed=model[range.clone()]!=value;
            let before=(model.clone(),b.view(v).sel().clone());
            model.replace_range(range.clone(),&value);
            let selection=Selection::from(Region::new(range.start,range.start+value.len()));
            b.transact(v,EditKind::Other,&[Edit{range,text:&value}],selection.clone());
            prop_assert_eq!(b.text().to_string(),model.as_str());prop_assert_eq!(b.view(v).sel(),&selection);
            if changed{steps.push((before,(model.clone(),selection)));}
        }
        for (before,_) in steps.iter().rev(){prop_assert!(b.undo(v));prop_assert_eq!(b.text().to_string(),before.0.as_str());prop_assert_eq!(b.view(v).sel(),&before.1);}
        prop_assert!(!b.undo(v));
        for (_,after) in &steps{prop_assert!(b.redo(v));prop_assert_eq!(b.text().to_string(),after.0.as_str());prop_assert_eq!(b.view(v).sel(),&after.1);}
        prop_assert!(!b.redo(v));
    }
    #[test]
    fn batched_transactions_match_sequential_string(initial in "[ab中🦀\n]{0,40}",raw in prop::collection::vec((any::<usize>(),any::<usize>(),"[xy世]{0,5}"),0..15)){
        let mut edits:Vec<_>=raw.iter().map(|(x,y,s)|{let a=boundary(&initial,*x);let b=boundary(&initial,*y);(a.min(b)..a.max(b),s.as_str())}).collect();edits.sort_by_key(|(r,_)|(r.start,r.end));
        let mut end=0;edits.retain(|(r,_)|{if r.start>=end{end=r.end;true}else{false}});
        let mut model=initial.clone();for (r,s) in edits.iter().rev(){model.replace_range(r.clone(),s);}
        let mut b=Buffer::new(initial.as_str());let v=b.new_view();let old=b.view(v).sel().clone();
        let batch:Vec<_>=edits.iter().map(|(r,s)|Edit{range:r.clone(),text:s}).collect();
        b.transact(v,EditKind::Other,&batch,Selection::default());prop_assert_eq!(b.text().to_string(),model.as_str());
        let changed=edits.iter().any(|(r,s)| &initial[r.clone()]!=*s);
        prop_assert_eq!(b.undo(v),changed);prop_assert_eq!(b.text().to_string(),initial);prop_assert_eq!(b.view(v).sel(),&old);prop_assert_eq!(b.redo(v),changed);prop_assert_eq!(b.text().to_string(),model);
    }
    #[test]
    fn random_position_maps_match_naive(commands in prop::collection::vec((0usize..80,0usize..80,0usize..12),1..80),seed in 0usize..80,right in any::<bool>()){
        let mut b=Buffer::new("a".repeat(80).as_str());let v=b.new_view();let origin=b.change_id();let mut model="a".repeat(80);let mut point=seed;
        for (x,y,n) in commands{let a=x%(model.len()+1);let z=y%(model.len()+1);let a=a.min(z);let z=x.max(y)%(model.len()+1);let end=a.max(z);let value="x".repeat(n);
            if model[a..end]!=value { point=naive_point(point,&[(a,end,n)],right); } model.replace_range(a..end,&value);
            b.transact(v,EditKind::Other,&[Edit{range:a..end,text:&value}],Selection::default());
            prop_assert_eq!(b.transform_point_from(seed,origin,if right{Bias::Right}else{Bias::Left}),Some(point));
        }
    }
    #[test]
    fn sorted_batch_mapping_matches_naive(raw in prop::collection::vec((0usize..100,0usize..8,0usize..8),0..20),p in 0usize..120,right in any::<bool>()){
        let mut items=raw;items.sort_by_key(|&(a,n,_)|(a,a+n));let mut end=0;let mut changes=Vec::new();for (a,n,new) in items{if a>=end && (changes.last().is_none_or(|&(old,_,_)|old!=a)){changes.push((a,a+n,new));end=a+n;}}
        let set=ChangeSet::new(changes.iter().map(|&(a,b,n)|Change{old_range:a..b,new_len:n}).collect());prop_assert_eq!(set.transform_point(p,if right{Bias::Right}else{Bias::Left}),naive_point(p,&changes,right));
    }
    #[test]
    fn normalization_matches_union_model(raw in prop::collection::vec((0usize..200,0usize..200),0..70)){
        let selection=Selection::from_regions(raw.iter().map(|&(a,b)|Region::new(a,b)));prop_assert!(!selection.is_empty());
        let mut previous=None;for r in selection.iter(){if let Some(end)=previous{prop_assert!(end<r.begin());}previous=Some(r.end());}
        for p in 0..200{let expected=raw.iter().any(|&(a,b)|a.min(b)<=p && p<=a.max(b)) || (raw.is_empty()&&p==0);prop_assert_eq!(selection.contains(p),expected);}
        if let Some(&(a,b))=raw.last(){prop_assert!(selection.primary().unwrap().contains(Region::new(a,b)));}
    }
    #[test]
    fn soft_selection_and_edit_roundtrip(initial in "[ab中🦀]{1,30}",moves in prop::collection::vec(any::<usize>(),1..35)){
        let mut b=Buffer::new(initial.as_str());let v=b.new_view();let start=b.view(v).sel().clone();let mut states=vec![start.clone()];
        for m in moves{let p=boundary(&initial,m);let s=Selection::from(Region::caret(p));if &s!=b.view(v).sel(){b.set_selection(v,s.clone());states.push(s);}}
        let before_edit=b.view(v).sel().clone();ops::insert(&mut b,v,"世");let final_text=b.text().to_string();let final_sel=b.view(v).sel().clone();
        prop_assert!(b.soft_undo(v));prop_assert_eq!(b.text().to_string(),initial.as_str());prop_assert_eq!(b.view(v).sel(),&before_edit);
        for expected in states.iter().rev().skip(1){prop_assert!(b.soft_undo(v));prop_assert_eq!(b.view(v).sel(),expected);}
        for expected in states.iter().skip(1){prop_assert!(b.soft_redo(v));prop_assert_eq!(b.view(v).sel(),expected);}
        prop_assert!(b.soft_redo(v));prop_assert_eq!(b.text().to_string(),final_text);prop_assert_eq!(b.view(v).sel(),&final_sel);
    }
    #[test]
    fn named_bias_and_drop_empty_match_model(a in 0usize..80,b in 0usize..80,start in 0usize..80,len in 0usize..15,n in 0usize..15,drop in any::<bool>()){
        let mut buffer=Buffer::new("x".repeat(100).as_str());let v=buffer.new_view();let region=Region::new(a,b);let end=(start+len).min(100);
        buffer.add_regions(v,"r",vec![region],drop);let old=buffer.change_id();let value="y".repeat(n);
        buffer.transact(v,EditKind::Other,&[Edit{range:start..end,text:&value}],Selection::default());
        let left=naive_point(region.begin(),&[(start,end,n)],true);let right=if region.empty(){left}else{naive_point(region.end(),&[(start,end,n)],false)};
        let expected=if a<=b{Region::new(left,right.max(left))}else{Region::new(right.max(left),left)};
        let mapped=buffer.transform_region_from(region,old).unwrap();prop_assert_eq!(mapped,expected);
        let set=buffer.view(v).get_regions("r").unwrap();if drop && expected.empty(){prop_assert!(set.regions.is_empty());}else{prop_assert_eq!(&set.regions,&vec![expected]);}
    }
}
#[test]
fn stale_and_future_ids_and_limits() {
    let mut b = Buffer::new("abc");
    let v = b.new_view();
    let old = b.change_id();
    b.set_history_limits(HistoryLimits {
        entries: 2,
        changes: 2,
        ..HistoryLimits::default()
    });
    for _ in 0..4 {
        b.break_undo_group();
        ops::insert(&mut b, v, "x");
    }
    assert_eq!(b.history_entries(), 2);
    assert!(b.transform_point_from(0, old, Bias::Right).is_none());
    assert!(
        b.transform_point_from(0, vel_buffer::ChangeId(999), Bias::Right)
            .is_none()
    );
    assert!(b.undo(v));
    assert!(b.undo(v));
    assert!(!b.undo(v));
}
proptest! {
    #[test]
    fn interleaved_soft_history_is_a_naive_state_stack(actions in prop::collection::vec((any::<bool>(),any::<usize>(),"[a中🦀 ]{1,3}"),1..70)){
        let mut b=Buffer::new("hello中");let v=b.new_view();let mut model="hello中".to_owned();let mut caret=0;let mut states=vec![(model.clone(),caret)];
        for (edit,at,value) in actions{
            if edit{model.insert_str(caret,&value);caret+=value.len();ops::insert(&mut b,v,&value);states.push((model.clone(),caret));}
            else{let p=boundary(&model,at);if p!=caret{caret=p;b.set_selection(v,Selection::from(Region::caret(p)));states.push((model.clone(),caret));}}
            prop_assert_eq!(b.text().to_string(),model.as_str());prop_assert_eq!(b.view(v).sel().primary().unwrap().b,caret);
        }
        for (text,p) in states.iter().rev().skip(1){prop_assert!(b.soft_undo(v));prop_assert_eq!(b.text().to_string(),text.as_str());prop_assert_eq!(b.view(v).sel().primary().unwrap().b,*p);}
        for (text,p) in states.iter().skip(1){prop_assert!(b.soft_redo(v));prop_assert_eq!(b.text().to_string(),text.as_str());prop_assert_eq!(b.view(v).sel().primary().unwrap().b,*p);}
    }
}
